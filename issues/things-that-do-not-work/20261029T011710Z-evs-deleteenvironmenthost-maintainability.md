Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ff73d0b7325d7e3e22aa47ea31b35af4c9a9d063

# The finished-record retention purge is written out three times in state.rs, once per record kind

_Filed: 2026-10-29T01:17:10Z_
_Operation: DeleteEnvironmentHost | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ff73d0b7325d7e3e22aa47ea31b35af4c9a9d063`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One rule — "at the cap, drop the record that has been FINISHED with longest, and never drop a live one" — is implemented three separate times in this crate's own state module, once per record kind:

* `StoredEnvironment::purge_finished_hosts` (state.rs:1640),
* `StoredEnvironment::purge_finished_connectors` (state.rs:1665),
* `ComAmazonawsEvsAmazonelasticvmwareserviceState::reclaim_environment_slot` + `longest_deleted_environment` (state.rs:2484, 2496).

The first two are verbatim copies of each other modulo the map (`hosts` / `connectors`), the terminal-state constant (`HOST_DELETED` / `CONNECTOR_DELETED`), the key field (`host_name` / `connector_id`) and the cap: the same `while len > cap` loop, the same `filter(terminal state)`, the same `min_by_key(modified_at_ms)`, the same `remove`. The crate itself records that they are copies — `purge_finished_hosts`'s own doc calls it "The host counterpart of [`Self::purge_finished_connectors`], and retained on the same terms" — and the third site states the same retention rule in prose on `MAX_ENVIRONMENTS` ("at the cap, the longest-deleted record is purged") while spelling the selection differently again (an explicit `(modified_at_ms, id)` tuple compare instead of `min_by_key`, and `len() < MAX` instead of `len() > MAX`).

Because the rule lives in three places, a change to it (how ties are broken, whether a record on its way out counts as finished, what the loop does when nothing is purgeable) has to be made three times, and the copies can drift apart silently — the spelling difference in the tie-break between the host/connector copies and the environment copy is already an instance of that divergence, even though the two spellings happen to select the same record today.

The fix is a single helper this module calls three times (one function parameterised by the map, the terminal predicate and the cap, or one generic over a small "finished record" trait). It is a pure refactor: every response and every existing test stays exactly as it is, because the selected record and the resulting store are identical.

## Evidence

Copy 1 — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1631-1653:
```
    /// Drop the oldest host records this environment is FINISHED with, until it holds no
    /// more than [`MAX_HOSTS_PER_ENVIRONMENT`].
    ///
    /// The host counterpart of [`Self::purge_finished_connectors`], and retained on the
    /// same terms: a `DELETED` host is kept so a later read still reports the deletion,
    /// ...
    pub fn purge_finished_hosts(&mut self) {
        while self.hosts.len() > MAX_HOSTS_PER_ENVIRONMENT {
            let Some(longest_deleted) = self
                .hosts
                .values()
                .filter(|host| host.state == HOST_DELETED)
                .min_by_key(|host| host.modified_at_ms)
                .map(|host| host.host_name.clone())
            else {
                return;
            };
            self.hosts.remove(&longest_deleted);
        }
    }
```

Copy 2 — the same lines with the connector nouns substituted, state.rs:1665-1678:
```
    pub fn purge_finished_connectors(&mut self) {
        while self.connectors.len() > MAX_CONNECTORS_PER_ENVIRONMENT {
            let Some(longest_deleted) = self
                .connectors
                .values()
                .filter(|connector| connector.state == CONNECTOR_DELETED)
                .min_by_key(|connector| connector.modified_at_ms)
                .map(|connector| connector.connector_id.clone())
            else {
                return;
            };
            self.connectors.remove(&longest_deleted);
        }
    }
```

Copy 3 — the same rule for the environment store, spelled differently, state.rs:2484-2505:
```
    pub fn reclaim_environment_slot(&mut self) {
        if self.environments.len() < MAX_ENVIRONMENTS {
            return;
        }
        if let Some(environment_id) = self.longest_deleted_environment() {
            self.environments.remove(&environment_id);
        }
    }

    /// The `environmentId` of the finished (`DELETED`) record that has been deleted
    /// longest, which is the one the retention rule reclaims first. ...
    fn longest_deleted_environment(&self) -> Option<String> {
        self.environments
            .values()
            .filter(|environment| environment.state == STATE_DELETED)
            .min_by(|left, right| {
                (left.modified_at_ms, left.environment_id.as_str())
                    .cmp(&(right.modified_at_ms, right.environment_id.as_str()))
            })
            .map(|environment| environment.environment_id.clone())
    }
```

The one rule all three state, written once on the cap constant — state.rs:1921-1927:
```
/// RETENTION RULE for the one record kind that is finished with: an environment that
/// has reached `DELETED` is kept so a later read still reports the deletion, but only
/// until the store needs the room — at the cap, the longest-deleted record is purged
```
and restated per copy at state.rs:1954-1957 ("The retention rule is the one [`MAX_ENVIRONMENTS`] states") and state.rs:1972-1975 (same sentence again for hosts).

The call sites, one per handler: services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/delete_environment_host.rs:277 (`environment.purge_finished_hosts();`), src/ops/delete_environment_connector.rs:310 (`environment.purge_finished_connectors();`), src/ops/create_environment.rs:838 (`state.reclaim_environment_slot();`).

Test coverage that must stay unchanged: tests/all/connector_deletion.rs:787-867 drives the connector copy to its cap and asserts "the longest-deleted record {purged} must have made room" against `evs::state::MAX_CONNECTORS_PER_ENVIRONMENT`; tests/all/environment_lifecycle.rs:1281-1336 drives the environment copy past `MAX_ENVIRONMENTS` and asserts a purged environment reads back absent; tests/all/host_deletion.rs:222-228 asserts the deleted host record is kept and still listed.

## Steps to Reproduce

1. Read services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1640-1653 and :1665-1678 side by side: the two bodies differ only in the map, the terminal-state constant, the key field and the cap.
2. Read state.rs:2484-2505: the same retention rule a third time, with the cap test written as `len() < MAX` and the selection as an explicit `(modified_at_ms, id)` tuple compare instead of `min_by_key`.
3. Read the doc on state.rs:1631-1635 ("The host counterpart of [`Self::purge_finished_connectors`], and retained on the same terms") and on state.rs:1954-1957 / :1972-1975 ("The retention rule is the one [`MAX_ENVIRONMENTS`] states"): the crate states one rule and then implements it once per record kind.
4. Observe that changing the rule — the tie-break, which states count as finished, or the behaviour when nothing is purgeable — requires editing three places that are already spelled two different ways, which is how the copies drift.
