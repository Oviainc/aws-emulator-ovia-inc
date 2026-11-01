Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# A multiplex alert is stamped with the clock of whichever call first noticed it, so a reporting call does change the id and SetTimestamp a later caller sees

_Filed: 2026-10-29T09:20:29Z_
_Operation: ListMultiplexAlerts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Multiplex alerts are raised inside `settle`, which every operation (reads included) runs first. The slice justifies that by stating the alert is owed to the STATE from the instant the mutation that created the condition ran, and that running it in a reporting operation "never changes what a later caller would otherwise have seen".

The code does not keep that promise: `apply_alerts` stamps the new record with `set_at: now` — the wall clock of the request that happened to settle first — and mints its `Id` out of the account-wide counter at that same moment. The condition itself became true earlier and at an instant the store already knows or could know: a multiplex reaches `RUNNING` at its stored `MultiplexTransition::due`, and a feeder disappearing is a mutation with its own instant.

So the stored alert, and therefore `ListMultiplexAlerts`, depends on the polling schedule rather than on state:
- start a multiplex with a program nothing feeds, wait past the transient span, and the alert's `setTimestamp` is the instant of the FIRST later call, not the instant the multiplex began transmitting — poll an hour late and the alert claims it was set an hour late;
- a reporting call that settles first consumes the next alert id from `IdMints::multiplex_alert`, so the ids a later caller sees for other multiplexes' alerts differ depending on whether anybody listed in between.

The sibling slice in this same crate does it the other way round: `SignalMapRecord::settle` deliberately stamps the DUE instant (`last_discovered_at = Some(self.discovery_due)`, `modified_at = due`) under a doc that gives exactly the reason the multiplex alerts are missing.

## Evidence

The claim — services/service-com-amazonaws-medialive-medialive/src/state/multiplexes.rs:1061-1071:
```
    /// Materialise every multiplex transition the clock had already made due as
    /// of `now`, then every alert the state it left behind calls for. Called
    /// from [`ComAmazonawsMedialiveMedialiveState::settle`], which every
    /// operation runs first.
    ///
    /// Raising and clearing alerts here is what keeps them owed to the STATE
    /// rather than to the call that happens to notice them: the condition was
    /// true, and the alert owed, from the instant the mutation that created it
    /// ran, whether or not anybody called afterwards. So a reporting operation
    /// may run this — it never changes what a later caller would otherwise have
    /// seen — and `ListMultiplexAlerts` is a pure read of the result.
```

The code under it — src/state/multiplexes.rs:984-1004:
```
        for condition in observed {
            if self
                .alerts
                .values()
                .any(|alert| alert.is_set() && alert.is_about(condition))
            {
                continue;
            }
            let id = FIRST_MULTIPLEX_ALERT_ID + mints.next(|mints| &mut mints.multiplex_alert);
            self.alerts.insert(
                id,
                MultiplexAlertRecord {
                    id,
                    alert_type: condition.alert_type,
                    program_name: condition.program_name.clone(),
                    message: condition.message.clone(),
                    set_at: now,
                    cleared_at: None,
                },
            );
        }
```
— `now` is the calling request's clock (`settle_multiplexes(now)`, src/state/multiplexes.rs:1092, reached from `state.settle(::std::time::SystemTime::now())` in src/ops/list_multiplex_alerts.rs:206), and the condition is derived purely from the settled state (`multiplex_alert_conditions`, src/state/multiplexes.rs:1853-1859: `if !multiplex.is_running() { return Vec::new(); }`), whose `RUNNING` was itself owed at the stored `transition.due` (src/state/multiplexes.rs:1040-1048).

The same crate's rule for a clock-owed stamp — src/state.rs:4839-4848:
```
    /// Materialise what the clock has already made due: the discovery, owed at
    /// `discovery_due`, and the outcome of a monitor deployment, owed at its own
    /// `due`. Both were owed at those instants whether or not anybody called, so
    /// applying them in a reporting operation changes nothing a later caller
    /// would have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.status == signal_map_status::CREATE_IN_PROGRESS && now >= self.discovery_due {
            self.status = signal_map_status::CREATE_COMPLETE.to_string();
            self.last_discovered_at = Some(self.discovery_due);
            self.modified_at = self.discovery_due;
```

and the store-wide contract the alert write breaks — src/state.rs:2443-2449:
```
    /// Materialise every transition the clock had already made due as of `now`,
    /// then answer from the result.
    ///
    /// Call this at the top of every operation, reporting ones included: it only
    /// applies transitions that were owed at that instant whether or not anybody
    /// called, so it never changes what a later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
```

The model member being reported: `MultiplexAlert$SetTimestamp` — "The time when the alert was set" (`smithy_query(query="shape", shape="MultiplexAlert")`), reported out of `set_at` (src/state/multiplexes.rs:1939 `multiplex_alert_output`).

Test coverage: the rule is stated by a test that only repeats a listing AFTER the alert exists — tests/all/multiplex_alerts.rs:1001-1003: "Rule: the listing is a report, so it never changes what a later caller sees — repeating it answers the same alerts, with the same ids and the same instants, and the multiplex it names is left where it was." (`listing_alerts_changes_nothing_a_later_caller_sees`, line 1008). No test covers the FIRST call to observe a condition, i.e. the one that mints the id and the timestamp; `assert_set_alert` only asserts that `setTimestamp` parses (tests/all/multiplex_alerts.rs:286).

## Steps to Reproduce

1. POST /prod/multiplexes with two availability zones and valid settings -> 201; wait for it to settle to IDLE. Note the multiplex id M.
2. POST /prod/multiplexes/M/programs with a programName P (nothing feeds it) -> 201.
3. POST /prod/multiplexes/M/start -> 202; the multiplex owes RUNNING at a stored due instant T.
4. Do nothing for a while after T, then GET /prod/multiplexes/M/alerts -> one PROGRAM_NO_RUNNING_CHANNEL alert whose setTimestamp is the instant of THIS call, not T.
5. Repeat the whole scenario but issue a GET /prod/multiplexes/M immediately after T and only then GET /prod/multiplexes/M/alerts -> the same alert now carries the earlier call's instant, i.e. the reporting call decided the stored SetTimestamp a later caller reads, contradicting src/state/multiplexes.rs:1066-1071 and src/state.rs:2446-2448.
6. With two multiplexes A and B both running with unfed programs, list A's alerts before B's and then (in a fresh run) list B's first: the ids each alert receives differ, because the id is minted from the account-wide `mints.multiplex_alert` at the moment of whichever call settled first.
