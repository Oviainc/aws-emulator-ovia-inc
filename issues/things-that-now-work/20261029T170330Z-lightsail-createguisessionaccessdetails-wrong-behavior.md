Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# CreateGUISessionAccessDetails opens the GUI session itself — starting its documented one-hour window — although the model says "Use StartGUISession to open the session" and the crate states that creating access details must not start an hour

_Filed: 2026-10-29T17:03:30Z_
_Operation: CreateGUISessionAccessDetails | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a running virtual computer that holds no session, `CreateGUISessionAccessDetails` calls `store.create_gui_session(..)`, which mints a `StoredGuiSession` whose `starts_at_ms` is now + one state transition and whose documented one-hour active window is measured from that instant (`expires_at_ms = starts_at_ms + GUI_SESSION_ACTIVE_MILLIS`). The answer is therefore `starting`, and the clock alone moves it to `started` and then to `startExpired` an hour later — with no `StartGUISession` call anywhere in the sequence.

The model splits the two operations the other way round: creating the access details creates the URLs, and "Use StartGUISession to open the session." The crate itself states that rule where the expiry is documented — the caller resumes an expired session with `StartGUISession`, "which is why creating access details does NOT silently start a second hour" — and the handler honours it for the EXPIRED case (it answers `startExpired` and mints nothing). The very same handler breaks it for the case where there is no session yet: it silently starts the FIRST hour.

The consequences are observable through this crate's own operations: (1) a caller that only reads the access details, never calling `StartGUISession`, has the hour run out anyway, after which the URLs are no longer reported; (2) `StartGUISession` on such a computer finds a live session and leaves it exactly as it is (src/ops/start_gui_session.rs:233-244), so the hour a caller believes it is starting is already part-spent; (3) the modelled `Status` member can never report `notStarted` for a running computer, although that is the state of a computer whose session nobody has opened.

## Evidence

The model, verbatim — smithy_query(query="doc", shape="com.amazonaws.lightsail#CreateGUISessionAccessDetails"):
> "Creates two URLs that are used to access a virtual computer's graphical user interface (GUI) session. ... Use StartGUISession to open the session."

The crate's own statement of the same rule — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:5409-5413:
```
/// The session's active window ([`GUI_SESSION_ACTIVE_MILLIS`]) has run out. The
/// caller resumes it with `StartGUISession` — "Use this action to resume the
/// session after it expires" (the modelled `StartGUISession` documentation) —
/// which is why creating access details does NOT silently start a second hour.
pub const GUI_SESSION_STATUS_START_EXPIRED: &str = "startExpired";
```

The handler that breaks it for the first hour — src/ops/create_gui_session_access_details.rs:243-257:
```
                    // Nothing is up yet: create the access details. The token
                    // comes from the store's own allocator, never from the
                    // request, and the store mints and writes it together, so a
                    // caller is never handed URLs for a session the store does
                    // not hold.
                    None => {
                        let Some(session) = store.create_gui_session(&name, now) else {
                            return Err(not_found(&name));
                        };
                        (
                            session.status_at(now),
                            session.percentage_complete_at(now),
                            Some(session.views(&name, endpoint.as_deref())),
                        )
                    }
```
what the mint sets, so the hour starts here — src/state.rs:5525-5539:
```
    pub fn starting_once_ready(token: String, now_ms: i64, ready_at_ms: i64) -> Self {
        Self {
            token,
            created_at_ms: now_ms,
            starts_at_ms: ready_at_ms
                .max(now_ms)
                .saturating_add(STATE_TRANSITION_MILLIS),
        }
    }
...
    pub fn expires_at_ms(&self) -> i64 {
        self.starts_at_ms.saturating_add(GUI_SESSION_ACTIVE_MILLIS)
    }
```
and the same handler obeying the rule two arms earlier, for a session whose hour has run out — src/ops/create_gui_session_access_details.rs:227-234:
```
                    // The session's documented active hour has run out, so its
                    // URLs reach nothing and are not reported. Resuming the
                    // session is `StartGUISession`'s business.
                    Some(session) if !session.is_live_at(now) => (
                        crate::state::GUI_SESSION_STATUS_START_EXPIRED,
```
the sibling that the model says opens a session, and which finds the hour already running — src/ops/start_gui_session.rs:226-244:
```
            // A session that is still inside its active window is the session
            // this call asked for: it is left exactly as it is, so a second
            // start neither mints a second session for one computer nor restarts
            // the window.
            let session = match live {
                Some(session) => session,
```

Test coverage: tests/all/gui_session_access_details.rs:303-308 pins the current behaviour ("a running virtual computer that holds no session access gets a pair of Amazon DCV URLs created for it ... reported `starting` until the clock owes the session `started`"), but its RULE restates the implementation rather than quoting a model sentence, and the model sentence it leaves out — "Use StartGUISession to open the session" — is the one the crate's own `GUI_SESSION_STATUS_START_EXPIRED` doc relies on for the expired case. No test calls CreateGUISessionAccessDetails and then checks that StartGUISession is what starts the hour.

## Steps to Reproduce

1. Call CreateInstances for "box" and wait for GetInstanceState to report running.
2. Call CreateGUISessionAccessDetails with resourceName="box" and NEVER call StartGUISession.
3. Observe status "starting" and a pair of session URLs: src/ops/create_gui_session_access_details.rs:248-251 minted the session, and src/state.rs:5525-5539 started its documented one-hour window at that instant.
4. Poll CreateGUISessionAccessDetails again: the status becomes "started" purely on the clock, and an hour after the mint it becomes "startExpired" and the URLs stop being reported — all without the StartGUISession call the model names as what opens a session.
5. Now call StartGUISession for "box" inside that hour: src/ops/start_gui_session.rs:233-244 finds the session live and leaves it unchanged, so the hour the caller is starting is already part-spent.
6. Read src/state.rs:5409-5413 ("creating access details does NOT silently start a second hour") next to src/ops/create_gui_session_access_details.rs:248 — the rule is applied to the second hour and broken for the first.
