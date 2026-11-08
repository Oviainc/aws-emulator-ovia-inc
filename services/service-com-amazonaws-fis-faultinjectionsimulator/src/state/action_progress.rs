// SPDX-License-Identifier: CC0-1.0

//! The SCHEDULE of the actions one run carries out: when each of them finishes, and so how
//! long the run itself takes.
//!
//! FIS starts an action as soon as every action its `startAfter` names has finished, so a
//! run lasts as long as the longest path through that graph ([`critical_path`], which
//! [`run_plan`](super::run_plan) compresses onto the emulator's run clock) and each action
//! occupies a span of it ([`finish_offset`], which
//! [`action_execution::plan_action_runs`](super::action_execution::plan_action_runs) lays
//! the run's actions out with). Both read the same walk, so the window the experiment
//! reports and the spans its actions report inside it cannot drift apart.
//!
//! It is one walk over the action map and nothing else: what carrying an action OUT means,
//! and what each one reports while the run is inside its span, is
//! [`action_execution`](super::action_execution)'s.

use super::*;

/// The longest path through `actions`, where each action's own length is the `duration` it
/// was asked for and an action that names none is instantaneous.
///
/// Walked ONCE per action, with each action's finish offset memoised and its own span
/// parsed exactly once up front, so the whole computation costs O(actions + `startAfter`
/// entries) — linear in the request, never in its square. A `startAfter` cycle, which no
/// stored template carries because [`actions_cycle_refusal`] refuses one, is broken at the
/// back edge rather than walked forever, so this terminates on any action map it is handed.
pub fn critical_path(actions: &HashMap<String, ExperimentTemplateAction>) -> Duration {
    // Parsed once per action, outside the walk.
    let spans: HashMap<&str, Duration> = actions
        .iter()
        .map(|(name, action)| (name.as_str(), action_span(action)))
        .collect();
    let mut finish: HashMap<&str, Duration> = HashMap::with_capacity(actions.len());
    let mut longest = Duration::ZERO;
    for name in actions.keys() {
        let offset = finish_offset(name.as_str(), actions, &spans, &mut finish);
        longest = longest.max(offset);
    }
    longest
}

/// When `root` finishes, measured from the start of the run: the latest finish of the
/// actions its `startAfter` names, plus its own span.
///
/// Resolved over an EXPLICIT stack rather than by recursion, so a long chain of
/// `startAfter` links costs heap entries and not call frames: the walk uses a constant
/// amount of stack whatever dependency chain the request declares. `finish` memoises the
/// answer for every action already resolved — across roots, so no action is totalled twice
/// — and `walking` holds the chain currently being resolved, so a back edge is answered as
/// "no dependency" instead of being followed. See [`critical_path`] for the cost this
/// keeps.
pub fn finish_offset<'a>(
    root: &'a str,
    actions: &'a HashMap<String, ExperimentTemplateAction>,
    spans: &HashMap<&'a str, Duration>,
    finish: &mut HashMap<&'a str, Duration>,
) -> Duration {
    // Each entry is an action and whether the actions it waits for have been pushed yet:
    // the SECOND visit of an action is the one that can total it, which is what taking the
    // latest finish of its dependencies needs.
    let mut stack: Vec<(&'a str, bool)> = vec![(root, false)];
    let mut walking: Vec<&'a str> = Vec::new();
    while let Some((name, dependencies_pushed)) = stack.pop() {
        if finish.contains_key(name) {
            continue;
        }
        if !dependencies_pushed {
            if walking.contains(&name) {
                continue;
            }
            walking.push(name);
            stack.push((name, true));
            for earlier in waits_for(name, actions) {
                stack.push((earlier, false));
            }
            continue;
        }
        let mut start = Duration::ZERO;
        for earlier in waits_for(name, actions) {
            // An action still unresolved here is one the walk broke a back edge at, which
            // contributes nothing.
            start = start.max(finish.get(earlier).copied().unwrap_or(Duration::ZERO));
        }
        // SATURATING: a template may name a `duration` of its own choosing for any action
        // other than `aws:fis:wait` (whose range `actions_refusal` checks), so a chain of
        // them can add up past what a `Duration` holds. The sum is capped there rather
        // than overflowing — a run that long is clamped to [`MAX_RUN_SPAN`] either way.
        let total = start.saturating_add(spans.get(name).copied().unwrap_or(Duration::ZERO));
        finish.insert(name, total);
        if walking.last() == Some(&name) {
            walking.pop();
        }
    }
    finish.get(root).copied().unwrap_or(Duration::ZERO)
}

/// The actions `name` waits for: the entries of its `startAfter` the action map holds.
///
/// `startAfter` names an action the template declares (`actions_refusal`); a name the map
/// does not hold contributes nothing rather than a guess.
fn waits_for<'a>(
    name: &str,
    actions: &'a HashMap<String, ExperimentTemplateAction>,
) -> impl Iterator<Item = &'a str> {
    actions
        .get(name)
        .into_iter()
        .flat_map(|action| action.start_after.iter().flatten())
        .filter_map(|earlier| {
            actions
                .get_key_value(earlier.as_str())
                .map(|(key, _)| key.as_str())
        })
}

/// How long one action takes: the span its `duration` parameter names, and nothing at all
/// for an action that carries none.
///
/// SOURCE: the AWS FIS actions reference, where `duration` is "the duration of the action"
/// (in ISO 8601 format) for every built-in action that takes it — `aws:fis:wait`, the
/// `aws:fis:inject-api-*` family, `aws:ebs:pause-volume-io`,
/// `aws:network:disrupt-connectivity`, … — so the parameter is read by its documented
/// meaning rather than for one action id. An action whose effect is a single API call
/// (`aws:ec2:reboot-instances`) takes no `duration` and so names no span; a parameter that
/// is some other span (`aws:ec2:stop-instances`' `startInstancesAfterDuration`) is not
/// this one and is not read as it.
///
/// A value that is not an ISO 8601 duration contributes nothing rather than a made-up
/// span: [`wait_duration`]'s own 1..720-minute range is NOT applied here, because that is
/// the range documented for `aws:fis:wait` and each other action publishes its own.
pub fn action_span(action: &ExperimentTemplateAction) -> Duration {
    action
        .parameters
        .as_ref()
        .and_then(|parameters| parameters.get(DURATION_PARAMETER))
        .and_then(|duration| iso8601_span(duration))
        .unwrap_or(Duration::ZERO)
}
