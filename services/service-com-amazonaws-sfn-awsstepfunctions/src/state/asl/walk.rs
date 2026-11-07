// SPDX-License-Identifier: CC0-1.0

//! The WALK: how one execution runs its definition, state by state.
//!
//! One concern, split out of [`super`] so that what a definition IS (its types, its
//! bounds, the members each state type carries) is read apart from what running one
//! DOES. The type is [`super::Interpreter`], declared beside those types in the module
//! root; this file carries the inherent `impl` that walks a state list — every state
//! type's own step, the data flow around it, its `Retry` and `Catch`, and the two
//! places a walk SUSPENDS (an activity task a worker claims, a token a callback
//! integration handed out).
//!
//! Every method here is `pub(super)`: the module root enters the walk
//! ([`super::interpret`]) and `TestState`'s single-state trial drives the same steps,
//! so both reach them, and nothing outside this module's own tree can.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value};

use super::*;

impl Interpreter<'_> {
    /// `value` as this service reports it: [`echoed_payload`] against the document
    /// the caller supplied to THIS walk.
    pub(super) fn echo(&self, value: &Value) -> String {
        echoed_payload(value, &self.supplied)
    }

    /// Whether this interpretation has run out of wall-clock time.
    ///
    /// The ceilings on steps, payloads, paths and nesting bound each factor of the
    /// work a definition can ask for; this bounds their PRODUCT, and it is what a
    /// run that is simply taking too long ends on.
    pub(super) fn out_of_time(&self) -> Option<Failure> {
        (super::super::now() > self.deadline).then(|| {
            runtime(format!(
                "the execution was interpreted for longer than the {} seconds this emulator \
                 interprets one definition for, so it was stopped",
                MAX_INTERPRET_DURATION.as_secs()
            ))
        })
    }

    /// Credit the `waited` a service-integration call took back to the deadline above.
    ///
    /// The walk is BLOCKED while a call it asked for is in flight — in another
    /// service's handler, which for a Lambda invocation runs a caller's own code — so
    /// that span is not interpretation and is not charged to the interpreter's own
    /// deadline. It is charged instead to what is left of [`MAX_CALL_WAIT_DURATION`],
    /// which bounds it across the whole walk: once that is spent, the time a further
    /// call takes falls on the deadline again, so the thread one run holds is bounded
    /// whatever its definition asks for.
    pub(super) fn spent_calling(&mut self, waited: Duration) {
        let credited = waited.min(self.call_wait_left);
        self.call_wait_left -= credited;
        self.deadline = super::super::after(self.deadline, credited);
    }

    /// Whether the definition's own `TimeoutSeconds` has run out by `at`, the offset
    /// into the run the next state would begin at.
    ///
    /// "The maximum number of seconds an execution of the state machine can run. If it
    /// runs longer than the specified time, the execution fails with a States.Timeout
    /// error name": the execution ENDS at that span, so a run still going there makes
    /// no further transition — and asks for no further work of another service — rather
    /// than running on to the emulator's own ceilings. What such a run is REPORTED as
    /// is the definition's timeout, which [`super::interpret`] answers for it.
    pub(super) fn past_timeout(&mut self, at: Duration) -> bool {
        if self.timeout_left.is_some_and(|left| at > left) {
            self.timed_out = true;
        }
        self.timed_out
    }

    /// Walk a NESTED state list — a `Map` iteration or a `Parallel` branch — one
    /// level deeper, refusing a definition that nests past [`MAX_NESTING_DEPTH`].
    ///
    /// `iteration` is whether the nested list is one `Map` ITEM's, which is what a
    /// run-a-job task inside it is refused by ([`Interpreter::map_depth`]).
    pub(super) fn nested_walk(
        &mut self,
        states: &Map<String, Value>,
        start_at: &str,
        input: Value,
        at: Duration,
        scope: &Scope,
        iteration: bool,
    ) -> Branch {
        if self.depth >= MAX_NESTING_DEPTH {
            return Branch {
                outcome: Err(runtime(format!(
                    "the definition nests 'Map' iterations and 'Parallel' branches more than the \
                     {MAX_NESTING_DEPTH} levels deep this emulator runs, so nothing was run for it"
                ))),
                end: at,
                suspended: false,
            };
        }
        self.depth += 1;
        if iteration {
            self.map_depth += 1;
        }
        // A nested machine's states carry their OWN retry counts; the enclosing
        // state's is not theirs.
        let enclosing = std::mem::take(&mut self.retry_count);
        // And their own VARIABLES: a nested machine's states read what reaching the
        // state that declares it assigned, and what they assign does not outlive the
        // machine, which is how the language scopes an `Assign`.
        let enclosing_variables = self.variables.clone();
        let branch = self.walk(states, start_at, input, at, scope);
        self.variables = enclosing_variables;
        self.retry_count = enclosing;
        if iteration {
            self.map_depth -= 1;
        }
        self.depth -= 1;
        branch
    }

    /// Whether this branch is one a SIBLING branch's failure has stopped, for a state
    /// about to be ENTERED at `at`, and the failure that says so.
    ///
    /// "If any branch fails, the entire Parallel state is considered to have failed and
    /// all the branches are stopped": the state's own outcome is the failure of the
    /// branch that failed ([`Interpreter::run_branches`] keeps the earliest), so this
    /// failure is never what a caller reads — what it decides is that the states of a
    /// stopped branch are not entered.
    ///
    /// The branches of a `Parallel` all BEGIN together, so a state due at the very
    /// instant a sibling failed had already begun: it is entered and then aborted, as
    /// every other state the failure caught in flight is. What a stopped branch does
    /// NOT do is the WORK of such a state ([`Interpreter::stopped_before_working`]).
    pub(super) fn stopped_before_entering(&self, at: Duration) -> Option<Failure> {
        self.stopped_by_a_sibling(at, false)
    }

    /// Whether this branch is one a sibling branch's failure has stopped, for the WORK
    /// of a state entered at `at`: the call a `Task` state would make.
    ///
    /// Work the failure caught IN FLIGHT does not finish — "a send that had not
    /// completed does not occur" — so the call of a state entered at or after that
    /// instant is not made at all, and the task is stopped instead.
    pub(super) fn stopped_before_working(&self, at: Duration) -> Option<Failure> {
        self.stopped_by_a_sibling(at, true)
    }

    /// The failure a stopped branch's state carries, when `at` is past the instant a
    /// sibling failed — `inclusive` for the work of a state, which the failure catches
    /// in flight, and exclusive for entering one.
    fn stopped_by_a_sibling(&self, at: Duration, inclusive: bool) -> Option<Failure> {
        let when = self.cancel_after?;
        (at > when || (inclusive && at == when)).then(|| {
            runtime(
                "a branch of the enclosing 'Parallel' state failed, so this branch was stopped"
                    .to_string(),
            )
        })
    }

    /// Walk `states` from `start_at`, with `input` as the first state's input and
    /// `at` as the offset into the run at which this branch begins.
    pub(super) fn walk(
        &mut self,
        states: &Map<String, Value>,
        start_at: &str,
        input: Value,
        at: Duration,
        scope: &Scope,
    ) -> Branch {
        let mut name = start_at.to_string();
        let mut current = input;
        let mut at = at;
        loop {
            // The definition's own `TimeoutSeconds` comes FIRST: an execution still
            // going when it falls due has ENDED there, so it makes no further
            // transition — and the emulator's own ceilings below never decide the
            // outcome of a run the definition already bounded.
            if self.past_timeout(at) {
                return Branch {
                    outcome: Err(Failure {
                        error: Some(super::TIMEOUT_ERROR.to_string()),
                        cause: None,
                    }),
                    end: at,
                    suspended: false,
                };
            }
            // A branch of the enclosing `Parallel` state has already FAILED, so the
            // state is failing and the work that had not begun does not begin: a state
            // due after that instant is not entered at all.
            if let Some(stopped) = self.stopped_before_entering(at) {
                return Branch {
                    outcome: Err(stopped),
                    end: at,
                    suspended: false,
                };
            }
            if let Some(failure) = self.out_of_time() {
                return Branch {
                    outcome: Err(failure),
                    end: at,
                    suspended: false,
                };
            }
            if self.budget == 0 {
                return Branch {
                    outcome: Err(runtime(format!(
                        "the execution made more than the {MAX_STATE_TRANSITIONS} state \
                         transitions this emulator runs, so it was stopped"
                    ))),
                    end: at,
                    suspended: false,
                };
            }
            self.budget -= 1;
            let Some(state) = states.get(&name).and_then(Value::as_object) else {
                return Branch {
                    outcome: Err(runtime(format!(
                        "state '{name}' is not declared by the state machine"
                    ))),
                    end: at,
                    suspended: false,
                };
            };
            // The raw input of the state about to run, kept for the one use a
            // FAILURE has for it: the step a redrive continues from is that state
            // with the data it saw ([`Run::stopped_at`]).
            let raw = current.clone();
            let step = match self.step(&name, state, current, at, scope) {
                Ok(step) => step,
                Err(fault) => {
                    // "It continues the failed execution from the unsuccessful step
                    // and uses the same input": the step is the TOP-LEVEL state of
                    // the definition the execution stopped at, so a failure a nested
                    // walk reports is recorded as the state that encloses it — the
                    // frame that is at depth zero.
                    if self.depth == 0 {
                        self.failed_at = Some(RedriveStep {
                            state_name: name.clone(),
                            state_input: raw.to_string(),
                        });
                    }
                    return Branch {
                        outcome: Err(fault.failure),
                        end: fault.at,
                        suspended: false,
                    };
                }
            };
            at = step.at;
            current = step.value;
            // The document this state produced is measured HERE, at the state that
            // produced it: a definition that grows what it carries is stopped at the
            // first state past what a state's output may hold, rather than after the
            // whole run.
            if let Err(failure) = measure_payload(&current, &format!("state '{name}'")) {
                return Branch {
                    outcome: Err(failure),
                    end: at,
                    suspended: false,
                };
            }
            match step.next {
                Next::EndBranch => {
                    return Branch {
                        outcome: Ok(current),
                        end: at,
                        suspended: false,
                    };
                }
                // The state scheduled an activity task, or started the job of a
                // run-a-job integration: the walk stops here, and `run` reports what
                // it is waiting for rather than an outcome. A nested list reports the
                // suspension to the state that encloses it, which is what parks a
                // `Parallel` branch at the job one of its states started.
                Next::Suspended => {
                    return Branch {
                        outcome: Ok(current),
                        end: at,
                        suspended: true,
                    };
                }
                Next::Named(next) => name = next,
            }
        }
    }

    /// Run one state: `raw` is its input, `at` the offset it is entered at.
    ///
    /// A diagnosis the interpreter raises about this state's own expression is
    /// reported under the state's name and the id of the event it was ENTERED at
    /// ([`diagnosed`]), which is read here — before the state records anything — so
    /// the state that failed can be followed back to its place in the history.
    pub(super) fn step(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        at: Duration,
        scope: &Scope,
    ) -> Result<StateStep, Fault> {
        let entered = self.next_event_id();
        self.run_state(name, state, raw, at, scope, entered)
            .map_err(|fault| diagnosed(name, entered, fault))
    }

    /// The id the next history event this walk records will carry.
    ///
    /// The walk's events are numbered in order from [`Interpreter::first_event_id`],
    /// so the id of the one about to be recorded is that first id plus the ones
    /// recorded before it. A walk that CONTINUES an execution from a reply reports
    /// only the steps that FOLLOW the point the reply was applied at
    /// ([`Resumed::index`]) — the ones before it were recorded by the call that
    /// suspended the execution — so those are the steps counted here.
    pub(super) fn next_event_id(&self) -> i64 {
        let recorded = self
            .steps
            .len()
            .saturating_sub(self.resumed.map_or(0, |resumed| resumed.index));
        self.first_event_id
            .saturating_add(i64::try_from(recorded).unwrap_or(i64::MAX))
    }

    /// [`Interpreter::step`]'s own work: `entered` is the id of the event this state
    /// records on being entered.
    #[allow(clippy::too_many_arguments)]
    fn run_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        at: Duration,
        scope: &Scope,
        entered: i64,
    ) -> Result<StateStep, Fault> {
        let state_type = state.get("Type").and_then(Value::as_str).unwrap_or_default();
        // A state's name is a caller-chosen object key that lands in every history
        // event of that state. `validate` refuses a longer one at create time; this
        // is the same bound made structural, so no name the history retains can be
        // longer than the Amazon States Language allows.
        if name.chars().count() > MAX_STATE_NAME_CHARS {
            return Err(runtime(format!(
                "a state name may carry at most {MAX_STATE_NAME_CHARS} characters, and this \
                 definition declares one of {}",
                name.chars().count()
            ))
            .at(at));
        }
        if !EXECUTED_STATE_TYPES.contains(&state_type) {
            return Err(runtime(format!(
                "state '{name}' is of type '{state_type}', which this emulator does not execute, \
                 so nothing was run for it"
            ))
            .at(at));
        }
        let language = self.language_of(state).map_err(|why| runtime(why).at(at))?;
        if let Some(member) = unexecutable_member(state_type, state, language) {
            return Err(runtime(format!(
                "state '{name}' carries the member '{member}', which this emulator does not \
                 interpret, so nothing was run for it"
            ))
            .at(at));
        }

        let context = self.context_for(scope, name, at);
        let effective = self
            .input_value(state, &raw, &context)
            .map_err(|failure| failure.clone().at(at))?;

        match state_type {
            "Pass" => {
                self.push_entered("PassStateEntered", name, at, &raw);
                // "Parameters ... Used to create a collection of key-value pairs
                // that will be passed as input": it transforms the state's effective
                // INPUT. `Result` is the state's RESULT, so a state that declares
                // both reports its `Result`, computed over the input `Parameters`
                // built. With neither, the result is the effective input unchanged.
                // Both members are JSONPath's: a JSONata `Pass` produces no result of
                // its own, so what it puts out is its `Output` over its own input.
                let result = match language {
                    JSONATA => None,
                    _ => {
                        let payload = match state.get("Parameters") {
                            Some(template) => payload_template(template, &effective, &context)
                                .map_err(|failure| failure.at(at))?,
                            None => effective.clone(),
                        };
                        Some(state.get("Result").cloned().unwrap_or(payload))
                    }
                };
                let value = self
                    .produced(state, language, &raw, &effective, result, &context, false)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("PassStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(name, state).map_err(|failure| failure.at(at))?,
                })
            }
            "Wait" => {
                self.push_entered("WaitStateEntered", name, at, &raw);
                let waited = self
                    .wait_duration(state, language, &effective, &context, at)
                    .map_err(|why| wait_failure(name, why).at(at))?;
                let at = at + waited;
                let value = self
                    .produced(state, language, &raw, &effective, None, &context, false)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("WaitStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(name, state).map_err(|failure| failure.at(at))?,
                })
            }
            "Choice" => {
                self.push_entered("ChoiceStateEntered", name, at, &raw);
                let chosen = self
                    .choose(state, language, &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                // "Output ... you can also specify Output in a Choice rule": the rule
                // that MATCHED decides the state's output where it declares one, and
                // the state's own `Output` otherwise.
                let value = match chosen.output {
                    Some(value) => value,
                    None => self
                        .produced(state, language, &raw, &effective, None, &context, false)
                        .map_err(|failure| failure.at(at))?,
                };
                self.push_exited("ChoiceStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::Named(chosen.next),
                })
            }
            "Succeed" => {
                self.push_entered("SucceedStateEntered", name, at, &raw);
                let value = self
                    .produced(state, language, &raw, &effective, None, &context, false)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("SucceedStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::EndBranch,
                })
            }
            "Fail" => {
                self.push_entered("FailStateEntered", name, at, &raw);
                // A `Fail` state ends the execution with the error and cause it
                // declares, both of which ASL leaves optional and either of which
                // may instead be read from the state's input — through a `*Path` in
                // JSONPath, through an expression in the member itself in JSONata.
                let error = self
                    .fail_member(state, language, "Error", "ErrorPath", &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                let cause = self
                    .fail_member(state, language, "Cause", "CausePath", &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                Err(Failure { error, cause }.at(at))
            }
            "Map" => self.map_state(name, state, language, raw, effective, &context, at),
            "Task" => self.task_state(name, state, language, raw, effective, &context, at, entered),
            // `Parallel` is the only type left: the list above is exhaustive.
            _ => self.parallel_state(name, state, language, raw, effective, &context, at),
        }
    }

    /// The query language one state is evaluated in: the one it declares, else the
    /// one the definition does.
    ///
    /// The `Err` is a `QueryLanguage` that names no query language the Amazon States
    /// Language defines, which `validate` refuses at create time.
    pub(super) fn language_of(&self, state: &Map<String, Value>) -> Result<&'static str, String> {
        Ok(declared_language(state.get("QueryLanguage"))?.unwrap_or(self.language))
    }

    /// The bindings a JSONata expression of this state is evaluated with.
    pub(super) fn bindings<'b>(
        &'b self,
        input: &'b Value,
        context: &'b Value,
        result: Option<&'b Value>,
        error_output: Option<&'b Value>,
    ) -> super::super::jsonata::Bindings<'b> {
        super::super::jsonata::Bindings {
            input,
            context,
            result,
            error_output,
            variables: &self.variables,
        }
    }

    /// What the expressions of ONE visit to a state may build and evaluate: the
    /// allowance is per visit, and it is spent against this interpretation's own
    /// deadline, so neither the member count of a template nor the time its
    /// expressions take is left to the definition.
    pub(super) fn jsonata_budget(&self) -> super::super::jsonata::Budget {
        super::super::jsonata::Budget::new(self.deadline)
    }

    /// What a state PUTS OUT, in the query language it is written in.
    ///
    /// JSONPath places the state's result through `ResultSelector`, `ResultPath` and
    /// `OutputPath`; JSONata evaluates the state's `Assign` and then its `Output`.
    /// One seam for both, so every state type's output follows the rule of the
    /// language that state is written in.
    ///
    /// `result` is what the state's own work produced, and `None` for a state that
    /// produces none and carries its input through (a `Pass`, a `Wait`, a `Choice`, a
    /// `Succeed`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn produced(
        &mut self,
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: &Value,
        result: Option<Value>,
        context: &Value,
        with_selector: bool,
    ) -> Result<Value, Failure> {
        if language == JSONATA {
            return self.jsonata_output(state, effective, result.as_ref(), None, context);
        }
        match result {
            Some(result) => self.place_result(state, raw, result, context, with_selector),
            None => self.output_value(state, effective.clone(), context),
        }
    }

    /// A JSONata state's `Assign`, and then the `Output` those assignments feed.
    ///
    /// "Assign ... store values in variables": the assignments are evaluated FIRST,
    /// so a variable this state assigns is in scope for its own `Output`, and they
    /// outlive the state — which is what a later state reads them through. A state
    /// that declares no `Output` puts out what its work produced, and its own input
    /// where its work produced nothing.
    ///
    /// A state that FAILS assigns nothing: where the `Output` those assignments feed
    /// cannot be built, the scope is put back as the state found it, so the states
    /// after a caught failure read what reaching the failed state had assigned and no
    /// more — and the event the state is left by reports no assignment of its own.
    pub(super) fn jsonata_output(
        &mut self,
        state: &Map<String, Value>,
        input: &Value,
        result: Option<&Value>,
        error_output: Option<&Value>,
        context: &Value,
    ) -> Result<Value, Failure> {
        // ONE allowance for the `Assign` and the `Output` it feeds: they are the
        // expressions of one template, so they spend from the same budget.
        let mut budget = self.jsonata_budget();
        let scope = self.variables.clone();
        let assigned = self.assigned.clone();
        let produced = self
            .jsonata_assign(state, input, result, error_output, context, &mut budget)
            .and_then(|()| match state.get("Output") {
                None => Ok(result.cloned().unwrap_or_else(|| input.clone())),
                Some(template) => {
                    let rendered = {
                        let bindings = self.bindings(input, context, result, error_output);
                        super::super::jsonata::render(template, &bindings, &mut budget)
                    };
                    rendered.map_err(jsonata_failure)
                }
            });
        if produced.is_err() {
            self.variables = scope;
            self.assigned = assigned;
        }
        produced
    }

    /// Apply the `Assign` of a state, a `Choice` rule or a `Catch` catcher.
    ///
    /// Each member is one variable, and they are evaluated in order, so an assignment
    /// reads the ones written before it. All of them spend from the ONE budget the
    /// visit was given, so a state cannot assign more than what a state's document
    /// may carry however many variables it declares.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn jsonata_assign(
        &mut self,
        node: &Map<String, Value>,
        input: &Value,
        result: Option<&Value>,
        error_output: Option<&Value>,
        context: &Value,
        budget: &mut super::super::jsonata::Budget,
    ) -> Result<(), Failure> {
        let Some(assignments) = node.get(ASSIGN_MEMBER) else {
            return Ok(());
        };
        let assignments = assignments.as_object().ok_or_else(|| {
            runtime(format!("'{ASSIGN_MEMBER}' must be a JSON object of variables"))
        })?;
        // The variables are built on a copy and installed once: an assignment that
        // fails leaves the scope as the state found it, so no later state reads half
        // of one state's assignments.
        let mut variables = self.variables.clone();
        let mut assigned = BTreeMap::new();
        for (variable, template) in assignments {
            let value = {
                let bindings = super::super::jsonata::Bindings {
                    input,
                    context,
                    result,
                    error_output,
                    variables: &variables,
                };
                super::super::jsonata::render(template, &bindings, budget)
                    .map_err(jsonata_failure)?
            };
            // A variable is a document the execution retains and a later state reads,
            // so it is measured against the same ceiling every document this walk
            // produces is measured against.
            measure_payload(&value, &format!("the variable '{variable}'"))?;
            assigned.insert(variable.clone(), value.to_string());
            variables.insert(variable.clone(), value);
        }
        self.variables = variables;
        // What THIS state assigned, for the event it is left by: a state that assigns
        // nothing reports nothing, whatever the scope around it holds.
        self.assigned.extend(assigned);
        Ok(())
    }

    /// A field that is either one JSONata expression or the literal it is written as,
    /// for a state written in that language; the literal itself for a JSONPath state.
    pub(super) fn literal(
        &self,
        language: &'static str,
        node: &Value,
        input: &Value,
        context: &Value,
    ) -> Result<Option<Value>, Failure> {
        if language != JSONATA {
            return Ok(Some(node.clone()));
        }
        let bindings = self.bindings(input, context, None, None);
        super::super::jsonata::field(node, &bindings, &mut self.jsonata_budget())
            .map_err(jsonata_failure)
    }

    /// The document a `Task` state hands its work.
    ///
    /// "Parameters ... Used to create a collection of key-value pairs that will be
    /// passed as input" — the member is `Arguments` in JSONata and `Parameters` in
    /// JSONPath, and a state that declares neither hands its work its own effective
    /// input.
    pub(super) fn task_request(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        effective: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match language {
            JSONATA => match state.get("Arguments") {
                Some(template) => {
                    let bindings = self.bindings(effective, context, None, None);
                    super::super::jsonata::render(template, &bindings, &mut self.jsonata_budget())
                        .map_err(jsonata_failure)
                }
                None => Ok(effective.clone()),
            },
            _ => match state.get("Parameters") {
                Some(template) => payload_template(template, effective, context),
                None => Ok(effective.clone()),
            },
        }
    }

    /// A `Task` state whose `Resource` names an ACTIVITY: the state SCHEDULES a task
    /// for that activity and the walk suspends, because what happens next is a
    /// worker's reply.
    ///
    /// "An activity is a worker — a program — that you host", which "polls Step
    /// Functions using the GetActivityTask API action and responds using SendTask*
    /// API actions": the task carries the state's effective input, and the state's
    /// own `TimeoutSeconds`/`HeartbeatSeconds` bound how long it may stay
    /// outstanding. A `Resource` naming any OTHER work is the service-integration
    /// call [`Interpreter::task_state`] makes, which is what hands this one the
    /// states it is for.
    ///
    /// A walk CONTINUING from a worker's reply to this state's task applies that reply
    /// here instead of suspending: the reply is the state's result, so the state's own
    /// result-side data flow places it and the walk goes on to its `Next`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn activity_task_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let resource = task_resource(state);
        if !super::super::names_an_activity(resource) {
            return Err(runtime(format!(
                "state '{name}' invokes '{}', a service integration this emulator does not make, \
                 so nothing was run for it",
                echoed(resource)
            ))
            .at(at));
        }
        // A nested state list is walked to its own end inside the enclosing state's
        // turn, so there is nowhere for one of its states to wait: a `Map` iteration
        // or a `Parallel` branch that schedules a task is not run, rather than run as
        // though the task had already answered.
        if self.depth > 0 {
            return Err(runtime(format!(
                "state '{name}' schedules an activity task inside a 'Map' iteration or a \
                 'Parallel' branch, which this emulator does not wait at, so nothing was run \
                 for it"
            ))
            .at(at));
        }
        // Both bounds are read from the state's effective INPUT, which is what their
        // `*Path` forms are reference paths into — not from the `Parameters` document
        // built below.
        let timeout_seconds = self
            .task_seconds(state, language, "TimeoutSeconds", &effective, context)
            .map_err(|failure| failure.at(at))?;
        let heartbeat_seconds = self
            .task_seconds(state, language, "HeartbeatSeconds", &effective, context)
            .map_err(|failure| failure.at(at))?;
        // "Parameters ... Used to create a collection of key-value pairs that will be
        // passed as input" — `Arguments` in JSONata: the document the template builds
        // IS what the worker is handed, and without one the state's effective input
        // is.
        let input = self
            .task_request(state, language, &effective, context)
            .map_err(|failure| failure.at(at))?;
        self.push_entered("TaskStateEntered", name, at, raw);
        self.push("ActivityScheduled", name, at);
        // A worker has already answered THIS state's task: the reply is that task's
        // result — "the JSON output of the task" — so the state's own result-side data
        // flow (`ResultSelector`, `ResultPath`, `OutputPath`) is applied to it and the
        // walk carries on to the state's `Next`, instead of suspending again. The
        // steps up to here are what the call that suspended the walk already recorded,
        // which is what [`Resumed::index`] marks.
        let replied = match &self.reply {
            Some(reply) if reply.state_name == name => self.reply.take(),
            _ => None,
        };
        if let Some(reply) = replied {
            self.resumed = Some(Resumed {
                index: self.steps.len(),
                at,
            });
            self.push("ActivitySucceeded", name, at);
            let value = self
                .produced(
                    state,
                    language,
                    raw,
                    &effective,
                    Some(reply.result),
                    context,
                    true,
                )
                .map_err(|failure| failure.at(at))?;
            self.push_exited("TaskStateExited", name, at, &value);
            let next = self.next_of(name, state).map_err(|failure| failure.at(at))?;
            return Ok(StateStep { value, at, next });
        }
        self.waiting = Some(ScheduledActivity {
            activity_arn: Some(resource.to_string()),
            // An activity's task is queued for a worker to claim, and the token that
            // names it is minted when the store queues it.
            token: None,
            state_name: name.to_string(),
            input: self.echo(&input),
            raw_input: raw.clone(),
            at,
            timeout_seconds,
            heartbeat_seconds,
            // An activity's task is answered by a worker rather than by a walk, so
            // its retriers are spent by the store ([`ActivityTask::retries_taken`])
            // and this record starts the first attempt with none spent.
            retries_taken: Vec::new(),
        });
        Ok(StateStep {
            value: input,
            at,
            next: Next::Suspended,
        })
    }

    /// A `Task` state's `TimeoutSeconds` or `HeartbeatSeconds`, from the literal
    /// member or from the `<member>Path` reference path into the state's input.
    ///
    /// Both are "a positive integer" of seconds, so a value that is not one is a
    /// defect in the definition rather than a bound to be guessed at.
    pub(super) fn task_seconds(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<u64>, Failure> {
        let path_member = format!("{member}Path");
        let declared = match (state.get(member), state.get(path_member.as_str())) {
            (Some(declared), _) => self.literal(language, declared, input, context)?,
            (None, Some(Value::String(path))) => Some(expression(path, input, context)?),
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'{path_member}' must be a JSONPath string, not {other}"
                )))
            }
            (None, None) => return Ok(None),
        };
        match declared.as_ref().and_then(Value::as_u64) {
            Some(seconds) if seconds > 0 => Ok(Some(seconds)),
            _ => Err(runtime(format!(
                "'{member}' must be a positive whole number of seconds"
            ))),
        }
    }

    /// The context object (`$$`) a state reads: the run's own members, plus the state
    /// it is being read in and the `Map` item being iterated, when there is one.
    pub(super) fn context_for(&self, scope: &Scope, name: &str, at: Duration) -> Value {
        let mut context = self.base_context.clone();
        if let Some(object) = context.as_object_mut() {
            object.insert(
                "State".to_string(),
                serde_json::json!({
                    "Name": name,
                    "EnteredTime": iso(self.start + at),
                    "RetryCount": self.retry_count,
                }),
            );
            if let Some(item) = &scope.map_item {
                object.insert("Map".to_string(), serde_json::json!({ "Item": item }));
            }
        }
        context
    }

    /// The context object (`$$`) the states of ONE child workflow execution read:
    /// this walk's own, with the `Execution` members of the CHILD.
    ///
    /// A Distributed Map's item runs as an execution in its own right, so the states
    /// inside it read that execution's identity — its ARN, its name, the input it was
    /// started with and the instant it started — rather than the parent's. The state
    /// machine stays the parent's: a child "runs the same state machine" the Map Run
    /// reports, which is what `ListMapRuns` names in `stateMachineArn`. It has been
    /// redriven no times, because it has just been started.
    pub(super) fn child_context(
        &self,
        arn: &str,
        name: &str,
        input: &Value,
        at: Duration,
    ) -> Value {
        let mut context = self.base_context.clone();
        if let Some(execution) = context.get_mut("Execution").and_then(Value::as_object_mut) {
            execution.insert("Id".to_string(), serde_json::json!(arn));
            execution.insert("Name".to_string(), serde_json::json!(name));
            execution.insert("Input".to_string(), input.clone());
            execution.insert(
                "StartTime".to_string(),
                serde_json::json!(iso(super::super::after(self.start, at))),
            );
            execution.insert("RedriveCount".to_string(), serde_json::json!(0));
        }
        context
    }

    /// A fresh name for one child workflow execution.
    ///
    /// Step Functions names a child workflow execution it starts for an item, so the
    /// name is drawn here, as a version-4 UUID — the same form, from the same
    /// generator, as the name the store mints for a `StartExecution` that gave none.
    /// The counter is what makes two draws in the same nanosecond differ.
    pub(super) fn fresh_child_name(&mut self) -> String {
        self.child_counter = self.child_counter.wrapping_add(1);
        let mut seed = super::super::now_nanos() ^ self.child_counter.rotate_left(29);
        let high = super::super::splitmix64(&mut seed);
        let low = super::super::splitmix64(&mut seed);
        super::super::uuid_v4(high, low)
    }

    /// A fresh `taskToken` for one callback (`.waitForTaskToken`) task of this walk.
    ///
    /// "Task tokens are generated by Step Functions when tasks are assigned to a
    /// worker, or in the context object when a workflow enters a task state": the
    /// token of a callback task is read out of the context object by the state's own
    /// parameters and leaves with the integration call, so it has to exist BEFORE
    /// that call is made and is drawn here rather than when the store records the
    /// task. It is built exactly as the token of an activity's task is
    /// ([`crate::state::task_token`]), over the execution this walk runs — which is
    /// what the token names for a task that belongs to no activity — and the id is
    /// drawn from the same generator a child execution's name is.
    pub(super) fn fresh_task_token(&mut self) -> String {
        self.child_counter = self.child_counter.wrapping_add(1);
        let mut seed = super::super::now_nanos() ^ self.child_counter.rotate_left(17);
        let high = super::super::splitmix64(&mut seed);
        let low = super::super::splitmix64(&mut seed);
        super::super::task_token(&super::super::uuid_v4(high, low), &self.execution_arn)
    }

    pub(super) fn push(&mut self, event_type: &'static str, state_name: &str, after: Duration) {
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::super::EventDetails::default(),
        );
    }

    /// The event a state records when it is ENTERED, carrying the data that state was
    /// entered WITH: "the string that contains the JSON input data for the state",
    /// which is the raw input the state received, before its own `InputPath` selects
    /// from it.
    pub(super) fn push_entered(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        input: &Value,
    ) {
        let input = self.echo(input);
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::super::EventDetails {
                input: Some(input),
                ..super::super::EventDetails::default()
            },
        );
    }

    /// The event a state records when it is LEFT, carrying the data that state EMITTED:
    /// "the JSON output data of the state", which is the document the state's own
    /// result-side data flow produced — `ResultSelector`, then `ResultPath`, then
    /// `OutputPath`, or a JSONata state's `Output` — and is the next state's input,
    /// beside the variables that are in scope as it leaves.
    pub(super) fn push_exited(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        output: &Value,
    ) {
        let output = self.echo(output);
        // "Map of variable name and value as a serialized JSON representation": what
        // the state being left ASSIGNED, which is what a caller reads a JSONata
        // state's `Assign` back through. A state that assigned none reports no map at
        // all, whatever the scope around it holds.
        let assigned_variables = std::mem::take(&mut self.assigned);
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::super::EventDetails {
                output: Some(output),
                assigned_variables,
                ..super::super::EventDetails::default()
            },
        );
    }

    /// One history event carrying what the walk knew about it.
    pub(super) fn push_detailed(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        details: super::super::EventDetails,
    ) {
        self.steps.push(Step {
            event_type,
            state_name: Some(state_name.to_string()),
            after,
            terminal: false,
            details,
        });
    }

    /// The event one attempt of a `Task` state's work failed with: the language's own
    /// timeout event where the work ran out of the time the state allowed it, and the
    /// task's own failure event otherwise, both carrying the work that failed and the
    /// error and cause it failed with.
    pub(super) fn push_task_failure(
        &mut self,
        state_name: &str,
        resource_type: &str,
        action: &str,
        failure: &Failure,
        after: Duration,
    ) {
        let event_type = if failure.name() == TIMEOUT_ERROR {
            super::super::EVENT_TASK_TIMED_OUT
        } else {
            super::super::EVENT_TASK_FAILED
        };
        let mut details = task_work_details(resource_type, action);
        details.error = failure.error.clone();
        details.cause = failure.cause.clone();
        self.push_detailed(event_type, state_name, after, details);
    }

    /// The event one attempt of a run-a-job task failed to START its job with: "the
    /// task submission failed", which is a different event from the failure of a task
    /// whose work ran — and the same event a request-response task records when ITS
    /// call failed, because for that pattern the call IS the work.
    pub(super) fn push_submit_failure(
        &mut self,
        state_name: &str,
        resource_type: &str,
        action: &str,
        failure: &Failure,
        after: Duration,
        call: &super::super::integration::Call,
    ) {
        if call.job.is_none() || failure.name() == TIMEOUT_ERROR {
            self.push_task_failure(state_name, resource_type, action, failure, after);
            return;
        }
        let mut details = task_work_details(resource_type, action);
        details.error = failure.error.clone();
        details.cause = failure.cause.clone();
        self.push_detailed(
            super::super::EVENT_TASK_SUBMIT_FAILED,
            state_name,
            after,
            details,
        );
    }

    /// What the job a run-a-job task started ENDED as, placed as that state's own
    /// outcome.
    ///
    /// The job is the task's unit of work, so a job that did its work makes the
    /// task succeed with the report the invoked service gave of it — placed by the
    /// state's own result-side data flow, exactly as a request-response call's
    /// response is — and a job that ended without doing it makes the task fail, which
    /// the state's `Catch` then answers for.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finished_job(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: &Value,
        work: (&str, &str),
        outcome: Result<Value, Failure>,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let (resource_type, action) = work;
        let outcome = outcome.and_then(|result| {
            measure_payload(&result, &format!("the 'Task' state '{name}'")).map(|_bytes| result)
        });
        match outcome {
            Ok(result) => {
                let mut details = task_work_details(resource_type, action);
                details.output = Some(result.to_string());
                self.push_detailed(super::super::EVENT_TASK_SUCCEEDED, name, at, details);
                let value = self
                    .produced(state, language, raw, effective, Some(result), context, true)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("TaskStateExited", name, at, &value);
                let next = self.next_of(name, state).map_err(|failure| failure.at(at))?;
                Ok(StateStep { value, at, next })
            }
            Err(failure) => {
                self.push_task_failure(name, resource_type, action, &failure, at);
                self.catch_or_fail(
                    (name, "TaskStateExited"),
                    state,
                    language,
                    raw,
                    effective,
                    failure,
                    context,
                    at,
                )
            }
        }
    }

    /// A `Task` state's failure, recorded on the state that produced it and then
    /// reported to the walk.
    /// The failure of a task is the TASK's, so the history attributes it to the work
    /// that failed — the error name and the cause, on the state that named it —
    /// whatever the walk then does with it: a caller reads what went wrong off the
    /// task rather than only off the execution the task ended.
    pub(super) fn task_fault(
        &mut self,
        state_name: &str,
        resource_type: &str,
        action: &str,
        failure: Failure,
        after: Duration,
    ) -> Fault {
        self.push_task_failure(state_name, resource_type, action, &failure, after);
        failure.at(after)
    }

    /// Where the walk goes after a state that carries `Next` or `"End": true`.
    pub(super) fn next_of(
        &self,
        name: &str,
        state: &Map<String, Value>,
    ) -> Result<Next, Failure> {
        if state.get("End").and_then(Value::as_bool) == Some(true) {
            return Ok(Next::EndBranch);
        }
        match state.get("Next").and_then(Value::as_str) {
            Some(next) => Ok(Next::Named(next.to_string())),
            None => Err(runtime(format!(
                "state '{name}' carries neither 'Next' nor \"End\": true"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Data flow: InputPath -> Parameters -> work -> ResultSelector -> ResultPath
    //            -> OutputPath
    // -----------------------------------------------------------------------

    /// The state's EFFECTIVE input: `InputPath` applied to the raw input.
    pub(super) fn input_value(
        &self,
        state: &Map<String, Value>,
        raw: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match state.get("InputPath") {
            None => Ok(raw.clone()),
            // "If the value of InputPath is null, that means that the raw input is
            // discarded, and the effective input for the state is an empty JSON
            // object, {}."
            Some(Value::Null) => Ok(serde_json::json!({})),
            Some(Value::String(path)) => select(path, raw, context),
            Some(other) => Err(runtime(format!(
                "'InputPath' must be a JSONPath string or null, not {other}"
            ))),
        }
    }

    /// `OutputPath` applied to a state's result.
    pub(super) fn output_value(
        &self,
        state: &Map<String, Value>,
        value: Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match state.get("OutputPath") {
            None => Ok(value),
            // "If the value of OutputPath is null, that means the input and result
            // are discarded, and the effective output from the state is an empty
            // JSON object, {}."
            Some(Value::Null) => Ok(serde_json::json!({})),
            Some(Value::String(path)) => select(path, &value, context),
            Some(other) => Err(runtime(format!(
                "'OutputPath' must be a JSONPath string or null, not {other}"
            ))),
        }
    }

    /// `ResultSelector`, then `ResultPath` against the state's RAW input, then
    /// `OutputPath`: the three steps that turn a state's result into its output.
    pub(super) fn place_result(
        &self,
        state: &Map<String, Value>,
        raw: &Value,
        result: Value,
        context: &Value,
        with_selector: bool,
    ) -> Result<Value, Failure> {
        let result = select_result(state, result, context, with_selector)?;
        let placed = path_result(state, raw, result)?;
        self.output_value(state, placed, context)
    }

    /// A `Fail` state's `Error`/`Cause`, from the literal member or from its `*Path`.
    ///
    /// In JSONata the member itself carries the expression — "you can use JSONata
    /// expressions in Error and Cause" — so there is no `*Path` form to read.
    pub(super) fn fail_member(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        path_member: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<String>, Failure> {
        if let Some(literal) = state.get(member) {
            let evaluated = self
                .literal(language, literal, input, context)?
                .unwrap_or(Value::Null);
            let text = evaluated.as_str().map(str::to_string).ok_or_else(|| {
                runtime(format!("a 'Fail' state's '{member}' must be a string"))
            })?;
            return Ok(Some(text));
        }
        match state.get(path_member) {
            None => Ok(None),
            Some(Value::String(path)) => {
                let value = expression(path, input, context)?;
                let text = value.as_str().map(str::to_string).ok_or_else(|| {
                    runtime(format!(
                        "'{path_member}' selected {value}, and a 'Fail' state's '{member}' must \
                         be a string"
                    ))
                })?;
                Ok(Some(text))
            }
            Some(other) => Err(runtime(format!(
                "'{path_member}' must be a JSONPath string, not {other}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Wait
    // -----------------------------------------------------------------------

    /// How long a `Wait` state waits.
    ///
    /// `Seconds` is the caller's own figure and keeps its value; `Timestamp` is an
    /// instant, so the wait is however much of it is still ahead of the run. Each has
    /// a `*Path` form that reads the figure from the state's input instead.
    pub(super) fn wait_duration(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        input: &Value,
        context: &Value,
        at: Duration,
    ) -> Result<Duration, String> {
        let evaluated = |node: &Value| -> Result<Option<Value>, String> {
            self.literal(language, node, input, context)
                .map_err(|failure| failure.cause.unwrap_or_default())
        };
        let seconds = match (state.get("Seconds"), state.get("SecondsPath")) {
            (Some(seconds), _) => evaluated(seconds)?,
            (None, Some(Value::String(path))) => Some(
                expression(path, input, context)
                    .map_err(|failure| failure.cause.unwrap_or_default())?,
            ),
            (None, Some(other)) => {
                return Err(format!("'SecondsPath' must be a JSONPath string, not {other}"));
            }
            (None, None) => None,
        };
        if let Some(seconds) = seconds {
            let seconds = seconds
                .as_u64()
                .ok_or_else(|| "'Seconds' must be a non-negative whole number".to_string())?;
            if seconds > MAX_WAIT_SECONDS {
                return Err(format!(
                    "'Seconds' is {seconds}, more than the {MAX_WAIT_SECONDS} seconds a 'Wait' \
                     state may wait for"
                ));
            }
            return Ok(Duration::from_secs(seconds));
        }

        let timestamp = match (state.get("Timestamp"), state.get("TimestampPath")) {
            (Some(timestamp), _) => evaluated(timestamp)?,
            (None, Some(Value::String(path))) => Some(
                expression(path, input, context)
                    .map_err(|failure| failure.cause.unwrap_or_default())?,
            ),
            (None, Some(other)) => {
                return Err(format!(
                    "'TimestampPath' must be a JSONPath string, not {other}"
                ));
            }
            (None, None) => None,
        };
        if let Some(timestamp) = timestamp {
            let text = timestamp
                .as_str()
                .ok_or_else(|| "'Timestamp' must be an ISO 8601 string".to_string())?;
            let due_millis = iso_millis(text)?;
            let reached = super::super::epoch_millis(self.start)
                .saturating_add(i64::try_from(at.as_millis()).unwrap_or(i64::MAX));
            // A `Timestamp` already behind the run is no wait at all; one further
            // ahead than a `Wait` may wait for is out of range, like an over-maximum
            // `Seconds`, and is NOT silently shortened to the maximum.
            let remaining = u64::try_from(due_millis.saturating_sub(reached)).unwrap_or(0);
            if remaining > MAX_WAIT_SECONDS.saturating_mul(1_000) {
                return Err(format!(
                    "'Timestamp' is {text}, further ahead than the {MAX_WAIT_SECONDS} seconds a \
                     'Wait' state may wait for"
                ));
            }
            return Ok(Duration::from_millis(remaining));
        }
        Err("a 'Wait' state must carry 'Seconds' or 'Timestamp'".to_string())
    }

    // -----------------------------------------------------------------------
    // Choice
    // -----------------------------------------------------------------------

    /// The state the first matching choice rule names, or the `Default`.
    ///
    /// Each rule is tested in the language the state is written in: a `Variable` and
    /// one of the comparators in JSONPath, a boolean `Condition` expression in
    /// JSONata. A JSONata rule may also carry an `Assign` and an `Output` of its own,
    /// which are the matching rule's — so they are applied here, where the rule that
    /// matched is known.
    pub(super) fn choose(
        &mut self,
        state: &Map<String, Value>,
        language: &'static str,
        input: &Value,
        context: &Value,
    ) -> Result<Chosen, Failure> {
        let choices = state
            .get("Choices")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                runtime("a 'Choice' state must carry a non-empty 'Choices' array".to_string())
            })?;
        if choices.is_empty() {
            return Err(runtime(
                "a 'Choice' state must carry a non-empty 'Choices' array".to_string(),
            ));
        }
        // Every rule is evaluated until one matches, so the rule count multiplies the
        // paths one visit to this state evaluates.
        if choices.len() > MAX_CHOICE_RULES {
            return Err(runtime(format!(
                "a 'Choice' state carries {} rules, more than the {MAX_CHOICE_RULES} this service \
                 evaluates",
                choices.len()
            )));
        }
        for rule in choices {
            let rule = rule.as_object().ok_or_else(|| {
                runtime("every member of 'Choices' must be a JSON object".to_string())
            })?;
            let matched = match language {
                JSONATA => {
                    let condition = rule.get("Condition").ok_or_else(|| {
                        runtime(format!(
                            "a 'Choice' rule written in '{JSONATA}' must carry a 'Condition'"
                        ))
                    })?;
                    let bindings = self.bindings(input, context, None, None);
                    super::super::jsonata::condition(
                        condition,
                        &bindings,
                        &mut self.jsonata_budget(),
                    )
                    .map_err(jsonata_failure)?
                }
                _ => condition_matches(rule, input, context)?,
            };
            if !matched {
                continue;
            }
            let next = rule.get("Next").and_then(Value::as_str).ok_or_else(|| {
                runtime("a top-level choice rule must carry a string 'Next'".to_string())
            })?;
            // The rule that matched is the one whose own `Assign` and `Output` apply;
            // a JSONPath rule carries neither. Both spend from one allowance, as a
            // state's own `Assign` and `Output` do.
            let output = match language {
                JSONATA => {
                    let mut budget = self.jsonata_budget();
                    self.jsonata_assign(rule, input, None, None, context, &mut budget)?;
                    match rule.get("Output") {
                        Some(template) => {
                            let bindings = self.bindings(input, context, None, None);
                            Some(
                                super::super::jsonata::render(template, &bindings, &mut budget)
                                    .map_err(jsonata_failure)?,
                            )
                        }
                        None => None,
                    }
                }
                _ => None,
            };
            return Ok(Chosen {
                next: next.to_string(),
                output,
            });
        }
        match state.get("Default").and_then(Value::as_str) {
            Some(default) => Ok(Chosen {
                next: default.to_string(),
                output: None,
            }),
            // A `Choice` that no rule matched and that declares no `Default` has
            // nowhere to go, so the execution ends at it — under the name the
            // interpreter raises every failure of its own under
            // ([`RUNTIME_ERROR`]), which is the name RECORDED for this outcome,
            // with the cause saying what could not be decided.
            None => Err(runtime(
                "no choice rule matched the state's input and the state declares no 'Default'"
                    .to_string(),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Task
    // -----------------------------------------------------------------------

    /// Run a `Task` state: do the work its `Resource` names, and place what came of
    /// it.
    ///
    /// A `Task` state "represents a single unit of work performed by a state
    /// machine", and the `Resource` says which work. An ACTIVITY belongs to a worker,
    /// so that one is scheduled and the walk suspends at it
    /// ([`Interpreter::activity_task_state`]). Every other `Resource` names a service
    /// INTEGRATION, whose unit of work is one API call: its request is the document
    /// the state's `Parameters` payload template builds over the state's effective
    /// input (the effective input itself when it declares none), and the operation's
    /// response document is the state's RESULT, which the result-side filters then
    /// place. A call that failed is the state's own failure, so the state's `Retry`
    /// and `Catch` apply to it exactly as they apply to a `Map`'s or a `Parallel`'s
    /// work — and what no call is made for is not faked, but reported as the failure
    /// it is.
    ///
    /// `entered` is the id of the event this state records on being entered, which the
    /// refusals that name this state report ([`executing_state`]).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn task_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
        entered: i64,
    ) -> Result<StateStep, Fault> {
        // The activity task is decided FIRST, before this state's own history event:
        // that path records its own, and the work there is a worker's rather than a
        // call this emulator makes.
        if super::super::names_an_activity(task_resource(state)) {
            return self
                .activity_task_state(name, state, language, &raw, effective, context, at);
        }
        self.push_entered("TaskStateEntered", name, at, &raw);
        let resource = state.get("Resource").and_then(Value::as_str).ok_or_else(|| {
            runtime(format!(
                "the 'Task' state '{name}' carries no string 'Resource', so it names no work"
            ))
            .at(at)
        })?;
        // "The service name of the resource in a task state" and "the action of the
        // resource called by a task state": what every one of this state's own events
        // names the work by, read once from the `Resource` they all describe.
        let (resource_type, action) = super::super::integration::named_work(resource);
        let call = super::super::integration::read(resource).map_err(|why| {
            self.task_fault(
                name,
                &resource_type,
                &action,
                runtime(format!("state '{name}': {why}")),
                at,
            )
        })?;
        // "TimeoutSeconds […] If the task runs longer than the specified seconds,
        // this state fails with a States.Timeout error name": the span it bounds is
        // the call's own, so the member is what the WAIT for the answer is bounded
        // by (`integration::Tasks::perform`), not merely what the span is compared
        // with afterwards. A state that declares none leaves the emulator's own
        // ceiling as the only bound.
        //
        // It is read through the SAME reader an activity task's bounds are read
        // with, which holds the member to the "positive integer" of seconds the
        // language defines: a figure that is not one is a defect in the definition,
        // and reading it as a whole number of seconds is what makes converting it to
        // a `Duration` total for every value a caller can write.
        let within = self
            .task_seconds(state, language, "TimeoutSeconds", &effective, context)
            .map_err(|failure| self.task_fault(name, &resource_type, &action, failure, at))?
            .map(Duration::from_secs);
        let heartbeat = self
            .task_seconds(state, language, "HeartbeatSeconds", &effective, context)
            .map_err(|failure| self.task_fault(name, &resource_type, &action, failure, at))?;
        // The job a run-a-job integration started has ENDED: what it ended as is this
        // state's own outcome, so the state reports it instead of starting the job a
        // second time. The events up to here are the ones the call that parked the
        // walk already recorded, which is what [`Resumed::index`] marks.
        let ended = match &self.job_reply {
            Some(reply) if reply.state_name == name => self.job_reply.take(),
            _ => None,
        };
        if let Some(ended) = ended {
            self.resumed = Some(Resumed {
                index: self.steps.len(),
                at,
            });
            return self.finished_job(
                name,
                state,
                language,
                &raw,
                &effective,
                (&resource_type, &action),
                ended.outcome,
                context,
                at,
            );
        }
        // A `Map` ITERATION is walked to its own end inside the enclosing state's
        // turn, and what the Map Run around it reports — the items it has finished,
        // the child execution each item ran as — is counted as those iterations
        // finish, which an iteration parked at a job has not done. So a run-a-job
        // task inside one is not run, rather than run as though the job had already
        // finished; a `Parallel` BRANCH parks, because a branch reports nothing until
        // it ends ([`ParkedFrame`]).
        if call.job.is_some() && self.map_depth > 0 {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                runtime(format!(
                    "state '{name}' starts a run-a-job integration inside a 'Map' iteration, \
                     which this emulator does not park at, so nothing was run for it"
                )),
                at,
            ));
        }
        // One execution parks at ONE job: the record that holds it names the state
        // that started it, and the poll that resolves it resumes that one state. A
        // second concurrent job is therefore not started either.
        if call.job.is_some() && self.parked.is_some() {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                runtime(format!(
                    "state '{name}' starts a second run-a-job integration while this execution \
                     is already parked at one, which this emulator does not park at, so nothing \
                     was run for it"
                )),
                at,
            ));
        }
        // A walk CONTINUING from a job that has ENDED has not reached the state that
        // job belongs to yet, so a task that would start a job here is one the attempt
        // this continues already started — the state machine has been updated under
        // the execution since. It is not started a second time.
        if call.job.is_some() && self.job_reply.is_some() {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                runtime(format!(
                    "state '{name}' would start the job of a run-a-job integration again while \
                     this execution is being continued from one that has ended, so nothing was \
                     run for it"
                )),
                at,
            ));
        }
        // The run-a-job pattern is served only for the STANDARD workflow type, so an
        // EXPRESS execution fails AT this state and the integration's call is NOT
        // made: no job is started for a task the workflow type cannot wait at.
        if call.job.is_some() && self.express {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                express_run_a_job_refusal(name, entered),
                at,
            ));
        }
        // A CALLBACK task is answered from outside the execution, which the walk can
        // only wait for where it is the walk itself that suspends: a nested state
        // list is walked to its own end inside the enclosing state's turn, so a `Map`
        // iteration or a `Parallel` branch whose state would wait for a token is not
        // run rather than run as though the token had already been answered. It is
        // the same bound an activity task is held to, for the same reason.
        if call.callback && self.depth > 0 {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                runtime(format!(
                    "state '{name}' waits for a task token inside a 'Map' iteration or a \
                     'Parallel' branch, which this emulator does not wait at, so nothing was \
                     run for it"
                )),
                at,
            ));
        }
        // And, like the run-a-job pattern, the callback pattern is served only for
        // the STANDARD workflow type: an EXPRESS execution fails AT this state and
        // the call is NOT made, so no token is handed out for an answer the workflow
        // type cannot wait for.
        if call.callback && self.express {
            return Err(self.task_fault(
                name,
                &resource_type,
                &action,
                express_callback_refusal(name, entered),
                at,
            ));
        }
        // A walk CONTINUING from an answer to this state's token takes up the retrier
        // budget the suspended task carried, so the state fails once its `Retry` is
        // spent instead of making the call again for every answer. Read BEFORE the
        // retry loop, because that loop's own first turn is what consumes the answer.
        self.resumed_attempts = self
            .callback_reply
            .as_ref()
            .filter(|reply| reply.state_name == name)
            .map(|reply| reply.retries_taken.clone());
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // A `SendTask*` has ANSWERED the callback this state's call carried: that
            // answer is this attempt's outcome, so the call is not made a second time
            // and the events up to here are the ones that call already recorded
            // ([`Resumed::index`]). A `Retry` that answers for a reported failure
            // runs the next attempt below, which makes the call — and hands out a
            // token — again.
            let answered = match &me.callback_reply {
                Some(reply) if reply.state_name == name => me.callback_reply.take(),
                _ => None,
            };
            if let Some(reply) = answered {
                me.resumed = Some(Resumed {
                    index: me.steps.len(),
                    at,
                });
                return match reply.outcome {
                    Ok(result) => {
                        let mut details = task_work_details(&resource_type, &action);
                        details.output = Some(me.echo(&result));
                        me.push_detailed(super::super::EVENT_TASK_SUCCEEDED, name, at, details);
                        let measured =
                            measure_payload(&result, &format!("the 'Task' state '{name}'"))
                                .map(|_bytes| result);
                        (measured, at)
                    }
                    Err(failure) => {
                        me.push_task_failure(name, &resource_type, &action, &failure, at);
                        (Err(failure), at)
                    }
                };
            }
            // A branch of the enclosing `Parallel` state has already failed, so this
            // state's work was stopped in flight: the call is NOT made, nothing is
            // scheduled for it, and nothing it would have done is done.
            if let Some(stopped) = me.stopped_before_working(at) {
                return (Err(stopped), at);
            }
            // This attempt's own request reads `$$.State.RetryCount`, so the payload
            // template is rendered with the context of the attempt it is on. A
            // callback attempt's request also reads `$$.Task.Token`, which is the
            // token THIS attempt hands the work it calls: each attempt is a separate
            // hand-over, so each gets a token of its own and only the last one
            // outstanding answers.
            let attempt_context = with_retry_count(context, me.retry_count);
            let token = call.callback.then(|| me.fresh_task_token());
            let attempt_context = match &token {
                Some(token) => with_task_token(&attempt_context, token),
                None => attempt_context,
            };
            let request = match me.task_request(state, language, &effective, &attempt_context) {
                Ok(request) => request,
                Err(failure) => {
                    me.push_task_failure(name, &resource_type, &action, &failure, at);
                    return (Err(failure), at);
                }
            };
            // Each attempt hands the work over and starts it again, so each writes
            // its own pair of events, carrying the document THAT attempt built and
            // the bounds the state declared for it.
            me.push_detailed(
                super::super::EVENT_TASK_SCHEDULED,
                name,
                at,
                super::super::EventDetails {
                    resource_type: Some(resource_type.clone()),
                    resource: Some(action.clone()),
                    input: Some(me.echo(&request)),
                    timeout_seconds: within.map(|within| within.as_secs()),
                    heartbeat_seconds: heartbeat,
                    ..super::super::EventDetails::default()
                },
            );
            // The call is made AS the state machine's execution role, so what that
            // role may do decides whether it is made AT ALL: a call the policies in
            // force on the role do not allow is the invoked service's own
            // access-denied refusal and this task's own failure, reported before the
            // request goes anywhere — so nothing is started for it and no submission
            // is claimed.
            if let Some(refused) = me.tasks.authorize(&call, &request) {
                let failure = Failure {
                    error: Some(refused.error),
                    cause: Some(refused.cause),
                };
                me.push_task_failure(name, &resource_type, &action, &failure, at);
                return (Err(failure), at);
            }
            me.push_detailed(
                super::super::EVENT_TASK_STARTED,
                name,
                at,
                task_work_details(&resource_type, &action),
            );
            // The interpretation's own deadline is consulted HERE as well as at each
            // transition: the walk blocks inside a call rather than transitioning, so
            // a run already past its deadline makes no further call (and a `Retry`
            // cannot keep making them).
            if let Some(failure) = me.out_of_time() {
                me.push_submit_failure(name, &resource_type, &action, &failure, at, &call);
                return (Err(failure), at);
            }
            let began = super::super::now();
            let answered = me.tasks.perform(&call, &request, within);
            // The span the walk spent BLOCKED in that call is not interpretation, so it
            // is credited back to the deadline and charged to the allowance that bounds
            // the waiting one run may do ([`Interpreter::spent_calling`]).
            me.spent_calling(
                super::super::now()
                    .duration_since(began)
                    .unwrap_or_default(),
            );
            // The identifier of the job a run-a-job call STARTED is read from the
            // answer as the invoked model spells it, before that answer becomes the
            // task's result in the convention the `Resource` reports it in
            // ([`super::super::integration::reported_result`]).
            let started = answered.as_ref().ok().and_then(|answer| {
                let job = call.job?;
                answer
                    .get(job.job_id)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            let attempt = match answered {
                // The result is a document a state carries, so it is measured
                // against the same ceiling every state's document is measured
                // against.
                Ok(answer) => {
                    let result = super::super::integration::reported_result(&call, &answer);
                    match measure_payload(&result, &format!("the 'Task' state '{name}'")) {
                        Ok(_bytes) => Ok(result),
                        Err(failure) => Err(failure),
                    }
                }
                Err(Failed { error, cause }) => Err(Failure {
                    error: Some(error),
                    cause: Some(cause),
                }),
            };
            match (&attempt, call.job) {
                // A run-a-job integration's call STARTED a job: the task is not
                // finished, so what the call answered is recorded as the task's
                // SUBMISSION ("the response from a resource when a task has
                // started") and the walk parks at the job until it ends.
                (Ok(result), Some(job)) => {
                    let Some(job_id) = started else {
                        let failure = runtime(format!(
                            "state '{name}': the '{}' call of {} answered no '{}', so the job it \
                             started cannot be reported and nothing was run for it",
                            call.operation, call.api.sdk_id, job.job_id
                        ));
                        me.push_submit_failure(name, &resource_type, &action, &failure, at, &call);
                        return (Err(failure), at);
                    };
                    let mut details = task_work_details(&resource_type, &action);
                    details.output = Some(result.to_string());
                    me.push_detailed(super::super::EVENT_TASK_SUBMITTED, name, at, details);
                    me.parked = Some(ParkedJob {
                        state_name: name.to_string(),
                        raw_input: raw.clone(),
                        job,
                        api: call.api,
                        parsed_output: call.parsed_output,
                        job_id: job_id.to_string(),
                        // The job is reported and stopped through calls of its own
                        // service, which for some integrations name it by a member of
                        // the request that STARTED it beside the identifier that call
                        // answered.
                        naming: job.naming(&request),
                        submitted: result.clone(),
                        at,
                        timeout_seconds: within.map(|within| within.as_secs()),
                        // Filled in as the walk unwinds: each concurrent state the
                        // suspension passes through records its own frame
                        // ([`Interpreter::run_branches`]).
                        frames: Vec::new(),
                    });
                }
                (Ok(result), None) => {
                    let mut details = task_work_details(&resource_type, &action);
                    details.output = Some(result.to_string());
                    // A CALLBACK integration's call HANDED the work over together
                    // with the token that answers it: the task is not finished, so
                    // what the call answered is recorded as the task's SUBMISSION
                    // ("the response from a resource when a task has started") and
                    // the walk suspends at the token until a `SendTask*` quotes it.
                    match &token {
                        Some(token) => {
                            me.push_detailed(
                                super::super::EVENT_TASK_SUBMITTED,
                                name,
                                at,
                                details,
                            );
                            me.waiting = Some(ScheduledActivity {
                                // A callback task belongs to no activity: no poll
                                // claims it, and what answers it is the token the
                                // call just carried.
                                activity_arn: None,
                                token: Some(token.clone()),
                                state_name: name.to_string(),
                                input: me.echo(&request),
                                raw_input: raw.clone(),
                                at,
                                timeout_seconds: within.map(|within| within.as_secs()),
                                heartbeat_seconds: heartbeat,
                                // The budget this state's `Retry` has spent so far,
                                // which the walk that continues from the answer takes
                                // up: the budget is the state's, not this attempt's.
                                retries_taken: me.attempts_spent.clone(),
                            });
                        }
                        None => {
                            me.push_detailed(
                                super::super::EVENT_TASK_SUCCEEDED,
                                name,
                                at,
                                details,
                            );
                        }
                    }
                }
                (Err(failure), _) => {
                    me.push_submit_failure(name, &resource_type, &action, failure, at, &call)
                }
            }
            (attempt, at)
        });
        match outcome {
            // The walk SUSPENDED at the token this state's callback call carried, or
            // PARKED at the job its run-a-job call started: the state has no result
            // yet, so the walk stops here and the execution waits for the answer. The
            // document the record carries is the one that call answered, which is what
            // the submission event reports.
            Ok(submitted) if self.parked.is_some() || self.waiting.is_some() => Ok(StateStep {
                value: submitted,
                at: end,
                next: Next::Suspended,
            }),
            Ok(result) => {
                let value = self
                    .produced(state, language, &raw, &effective, Some(result), context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("TaskStateExited", name, end, &value);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self
                        .next_of(name, state)
                        .map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => self.catch_or_fail(
                (name, "TaskStateExited"),
                state,
                language,
                &raw,
                &effective,
                failure,
                context,
                end,
            ),
        }
    }

    // -----------------------------------------------------------------------
    // Map
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub(super) fn map_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        self.push_entered("MapStateEntered", name, at, &raw);
        // Which Map state of this walk this one is, in ENTRY order: the Map Run it may
        // start is recorded once its items are done, and the list it goes in is read
        // in this order rather than that one.
        let entered = self.map_runs_entered;
        self.map_runs_entered = self.map_runs_entered.saturating_add(1);
        let (processor, distributed) = map_processor(name, state).map_err(|failure| failure.at(at))?;
        let (processor_states, processor_start) =
            states_of(processor, &format!("the 'ItemProcessor' of state '{name}'"))
                .map_err(|failure| failure.at(at))?;
        // "A user-defined or an auto-generated string that identifies a Map state":
        // the field the Map Run's own ARN carries, and the field the ARN of every
        // child execution it starts carries too, so one reading serves both.
        let label = state
            .get("Label")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
        // "ItemBatcher — specifies how to batch the items": a state that declares one
        // hands each iteration a BATCH of its items rather than one item, and the
        // `ItemSelector` is applied to each item on the way into its batch.
        let batcher = state.get("ItemBatcher");
        let concurrency = self
            .number_member(state, language, "MaxConcurrency", &effective, context, 0.0)
            .map_err(|failure| failure.at(at))? as usize;
        let tolerated_count = self
            .number_member(
                state,
                language,
                "ToleratedFailureCount",
                &effective,
                context,
                0.0,
            )
            .map_err(|failure| failure.at(at))? as u64;
        let tolerated_percentage = self
            .number_member(
                state,
                language,
                "ToleratedFailurePercentage",
                &effective,
                context,
                0.0,
            )
            .map_err(|failure| failure.at(at))?;
        let tolerance_declared = [
            "ToleratedFailureCount",
            "ToleratedFailureCountPath",
            "ToleratedFailurePercentage",
            "ToleratedFailurePercentagePath",
        ]
        .iter()
        .any(|member| state.contains_key(*member));

        // The tally belongs to the LAST attempt: a retried `Map` state resets it, so
        // what is reported is what the attempt the state ended on did.
        let mut tally = MapTally::default();
        // How many items the attempt the state ended on iterated, which is what the Map
        // Run reports: every attempt reads the state's item source again, so the figure
        // belongs to the attempt rather than to the state.
        let mut total_items = 0usize;
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // Every attempt at this state reads its item source again — a `Retry`
            // re-runs the state, and reading the dataset an `ItemReader` names is part
            // of what the state DOES — so a read that failed is this attempt's failure
            // and the next attempt reads it afresh.
            let read = me
                .map_items(name, state, language, &effective, context)
                .and_then(|items| match batcher {
                    None => Ok(items),
                    Some(batcher) => me.batched_items(
                        name, state, language, batcher, items, &effective, context,
                    ),
                });
            let items = match read {
                Ok(items) => items,
                Err(failure) => return (Err(failure), at),
            };
            total_items = items.len();
            // Every attempt iterates those items, so every attempt writes its own
            // `MapStateStarted` — carrying "the size of the array for Map state
            // iterations" — and the event of the outcome those iterations reached.
            me.push_detailed(
                super::super::EVENT_MAP_STATE_STARTED,
                name,
                at,
                super::super::EventDetails {
                    length: Some(i32::try_from(items.len()).unwrap_or(i32::MAX)),
                    ..super::super::EventDetails::default()
                },
            );
            let plan = MapPlan {
                items: &items,
                name,
                label: &label,
                states: processor_states,
                start_at: processor_start,
                selector: match batcher {
                    // A batched state's selector was spent on the items BEFORE they
                    // were placed in their batches, so the batch an iteration is handed
                    // is already the document it runs on.
                    Some(_) => None,
                    None => state.get("ItemSelector").or_else(|| state.get("Parameters")),
                },
                language,
                distributed,
                batched: batcher.is_some(),
                concurrency,
                tolerated_count,
                tolerated_percentage,
                tolerance_declared,
            };
            // This attempt's own members read `$$.State.RetryCount`, so they are
            // evaluated with the context of the attempt they are on.
            let context = with_retry_count(context, me.retry_count);
            let (outcome, end) = me.run_map_items(&plan, &effective, &context, at, &mut tally);
            let event = if outcome.is_ok() {
                super::super::EVENT_MAP_STATE_SUCCEEDED
            } else {
                super::super::EVENT_MAP_STATE_FAILED
            };
            me.push(event, name, end);
            (outcome, end)
        });
        // A Distributed Map "starts a Map Run", a resource of its own; an INLINE Map
        // starts none, so only the distributed mode records one.
        if distributed {
            // Held in the order the iterations FINISHED, which is the order the Map
            // Run counts them in: items of one batch run at once, so the one that
            // finished first is not always the one that started first.
            let mut finished_items = std::mem::take(&mut tally.items);
            finished_items.sort_by_key(|item| item.finished_at);
            // Where this state's Map Run belongs in the walk's list is decided by the
            // position its state holds among the Map states ENTERED, never by a
            // position in the list captured before the items ran: the same list is
            // pruned WHILE they run, by the failure of a nested `Parallel` or `Map`
            // ([`Interpreter::abort_units`]), so a position taken at entry no longer
            // names the same place.
            self.map_runs.push((
                entered,
                StartedMapRun {
                    label: label.clone(),
                    status: if outcome.is_ok() {
                        super::super::MAP_RUN_STATUS_SUCCEEDED
                    } else {
                        super::super::MAP_RUN_STATUS_FAILED
                    },
                    started_at: at,
                    stopped_at: end,
                    max_concurrency: super::super::reported_max_concurrency(
                        i64::try_from(concurrency).unwrap_or(i64::MAX),
                    ),
                    tolerated_failure_count: i64::try_from(tolerated_count).unwrap_or(i64::MAX),
                    tolerated_failure_percentage: tolerated_percentage as f32,
                    total_items: i64::try_from(total_items).unwrap_or(i64::MAX),
                    items: finished_items,
                    // One child execution per item of the attempt the state ended
                    // on, which is what `ListExecutions` reports for this Map Run.
                    children: std::mem::take(&mut tally.children),
                },
            ));
        }
        match outcome {
            Ok(results) => {
                let value = self
                    .produced(state, language, &raw, &effective, Some(results), context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("MapStateExited", name, end, &value);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self
                        .next_of(name, state)
                        .map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => self.catch_or_fail(
                (name, "MapStateExited"),
                state,
                language,
                &raw,
                &effective,
                failure,
                context,
                end,
            ),
        }
    }

    /// One `Map` state's `ItemSelector`, in the query language that state is written
    /// in: a payload template in JSONPath, a template of JSONata expressions in
    /// JSONata.
    pub(super) fn render_selector(
        &self,
        language: &'static str,
        template: &Value,
        effective: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match language {
            JSONATA => {
                let bindings = self.bindings(effective, context, None, None);
                super::super::jsonata::render(template, &bindings, &mut self.jsonata_budget())
                    .map_err(jsonata_failure)
            }
            _ => payload_template(template, effective, context),
        }
    }

    /// The items a `Map` state iterates: what its `ItemReader`'s dataset holds when it
    /// declares one, else `Items` when it declares that, else what `ItemsPath`
    /// (default `$`) selects from the state's effective input.
    pub(super) fn map_items(
        &self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        effective: &Value,
        context: &Value,
    ) -> Result<Vec<Value>, Failure> {
        // "ItemReader — specifies a dataset and its location": a state that declares
        // one iterates THAT dataset, so nothing of its own input is read for its items.
        if let Some(reader) = state.get("ItemReader") {
            return self.dataset_items(name, reader, language, effective, context);
        }
        let selected = match state.get("Items") {
            // "Items — an array, or a JSONata expression that evaluates to an array":
            // the member is the array itself in either language, and in JSONata it may
            // be the expression that produces one.
            Some(items) => match language {
                JSONATA => self
                    .literal(language, items, effective, context)?
                    .unwrap_or(Value::Null),
                _ => payload_template(items, effective, context)?,
            },
            None => match state.get("ItemsPath") {
                None => effective.clone(),
                Some(Value::String(path)) => select(path, effective, context)?,
                Some(other) => {
                    return Err(runtime(format!(
                        "'ItemsPath' must be a reference-path string, not {other}"
                    )));
                }
            },
        };
        match selected {
            Value::Array(items) => Ok(items),
            other => Err(runtime(format!(
                "the 'Map' state '{name}' was given {other}, which is not the array of items a \
                 'Map' state iterates"
            ))),
        }
    }

    /// The items the DATASET a `Map` state's `ItemReader` names holds.
    ///
    /// The reader's own `Parameters` (or JSONata `Arguments`) is a payload template of
    /// the state's language, so it is rendered here — against the state's effective
    /// input, exactly as its `ItemSelector` is — and `MaxItems` is resolved from the
    /// member or from its `MaxItemsPath` form in that same language. The dataset is
    /// then read through the same seam a `Task` state's integration call is made over
    /// ([`super::super::item_reader`]).
    pub(super) fn dataset_items(
        &self,
        name: &str,
        reader: &Value,
        language: &'static str,
        effective: &Value,
        context: &Value,
    ) -> Result<Vec<Value>, Failure> {
        let parameters = match reader.get("Arguments").or_else(|| reader.get("Parameters")) {
            Some(template) => self.render_selector(language, template, effective, context)?,
            None => Value::Object(Map::new()),
        };
        // "MaxItems — limits the number of data items passed", and `MaxItemsPath` reads
        // that figure from the state's input instead: one member of the reader's
        // `ReaderConfig`, resolved the same way every other numeric member of a state
        // is.
        let empty = Map::new();
        let config = reader
            .get("ReaderConfig")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let max_items =
            self.number_member(config, language, "MaxItems", effective, context, 0.0)? as usize;
        let dataset = super::super::item_reader::read(name, reader, &parameters, max_items)
            .map_err(reported)?;
        dataset.items(name, self.tasks).map_err(reported)
    }

    /// `items` grouped as the state's `ItemBatcher` says, for a state that declares
    /// one: each of the documents the state's iterations are then handed is one BATCH.
    ///
    /// The `ItemSelector` is applied FIRST, item by item, because it transforms each
    /// item and the batches are built out of what it produced — so the state's own
    /// selector is spent here and the iterations are handed the batches unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn batched_items(
        &self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        batcher: &Value,
        items: Vec<Value>,
        effective: &Value,
        context: &Value,
    ) -> Result<Vec<Value>, Failure> {
        let Some(members) = batcher.as_object() else {
            return Err(runtime(format!(
                "'ItemBatcher' must be an object, not {batcher}"
            )));
        };
        if let Some(member) = members
            .keys()
            .find(|member| !super::super::item_batcher::BATCHER_MEMBERS.contains(&member.as_str()))
        {
            return Err(runtime(format!(
                "the 'ItemBatcher' carries the member '{member}', which this emulator does not \
                 interpret, so nothing was run for it"
            )));
        }
        let selected = match state.get("ItemSelector").or_else(|| state.get("Parameters")) {
            None => items,
            Some(template) => {
                let mut selected = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    let map_item = serde_json::json!({ "Index": index, "Value": item });
                    let item_context = with_map_item(context, &map_item);
                    selected.push(
                        self.render_selector(language, template, effective, &item_context)?,
                    );
                }
                selected
            }
        };
        let batching = super::super::item_batcher::Batching {
            max_items: self.number_member(
                members,
                language,
                "MaxItemsPerBatch",
                effective,
                context,
                0.0,
            )? as usize,
            max_bytes: self.number_member(
                members,
                language,
                "MaxInputBytesPerBatch",
                effective,
                context,
                0.0,
            )? as usize,
            batch_input: match members.get("BatchInput") {
                None => None,
                Some(template) => {
                    Some(self.render_selector(language, template, effective, context)?)
                }
            },
        };
        super::super::item_batcher::batches(name, selected, &batching).map_err(reported)
    }

    /// Run every item of a `Map` state and collect what the iterations produced.
    ///
    /// Items run concurrently on the real service, in batches of `MaxConcurrency`, so
    /// a batch's span is its longest item's rather than the sum: that is what the
    /// returned offset reports. The ITEMS themselves are interpreted one after the
    /// other, because the interpreter is a single walk — the result is the same, and
    /// no iteration can observe another's.
    ///
    /// `tally` is counted up as the items are run, so it holds what this attempt's
    /// items did however the attempt ended.
    pub(super) fn run_map_items(
        &mut self,
        plan: &MapPlan<'_>,
        effective: &Value,
        context: &Value,
        at: Duration,
        tally: &mut MapTally,
    ) -> Attempt {
        // As for a `Parallel`'s branches: what stopped the walk AROUND this state goes
        // on stopping it, and what one of this state's own iterations stops does not
        // outlive the state ([`Interpreter::cancel_after`]).
        let enclosing = self.cancel_after;
        let attempt = self.walk_map_items(plan, effective, context, at, tally);
        self.cancel_after = enclosing;
        attempt
    }

    /// [`Interpreter::run_map_items`]' own work, inside the stop it restores.
    fn walk_map_items(
        &mut self,
        plan: &MapPlan<'_>,
        effective: &Value,
        context: &Value,
        at: Duration,
        tally: &mut MapTally,
    ) -> Attempt {
        *tally = MapTally::default();
        let batch = if plan.concurrency == 0 {
            plan.items.len().max(1)
        } else {
            plan.concurrency
        };
        let mut results: Vec<Value> = Vec::new();
        let mut results_bytes = 0usize;
        let mut failures: u64 = 0;
        // The iteration that failed FIRST, with the offset it failed at: the failure
        // an intolerant state reports, and the instant its other iterations stopped.
        let mut first_failure: Option<(Failure, Duration)> = None;
        // Where each iteration's own events begin in this walk's list, so an iteration
        // the state's failure aborted can be cut back to the instant it failed.
        let mut units: Vec<Unit> = Vec::new();
        // "ToleratedFailureCount" and "ToleratedFailurePercentage" are what lets a
        // state go on iterating past a failed item; a state that declares neither
        // tolerates none, so the first failure is the state's own and the iterations
        // that had not finished stop there.
        let tolerant = plan.distributed || plan.tolerance_declared;
        let mut at = at;
        let mut index = 0usize;
        for chunk in plan.items.chunks(batch) {
            let batch_start = at;
            let mut batch_end = at;
            for item in chunk {
                // Every item's turn is charged to the step budget BEFORE the work it
                // needs is done, so the per-item work (the context the iteration
                // reads, the `ItemSelector` it is given) stops with the budget rather
                // than being done for every remaining item of a long array.
                if let Some(failure) = self.out_of_time() {
                    return (Err(failure), batch_end);
                }
                if self.budget == 0 {
                    return (
                        Err(runtime(format!(
                            "the execution made more than the {MAX_STATE_TRANSITIONS} state \
                             transitions this emulator runs, so it was stopped"
                        ))),
                        batch_end,
                    );
                }
                // `$$.Map.Item` is what the iteration reads the item and its
                // position through, both in the `ItemSelector` evaluated here and in
                // every state of the iteration.
                let map_item = serde_json::json!({ "Index": index, "Value": item });
                let item_context = with_map_item(context, &map_item);
                let scope = Scope {
                    map_item: Some(map_item),
                };
                let item_input = match plan.selector {
                    Some(template) => match self.render_selector(plan.language, template, effective, &item_context) {
                        Ok(value) => value,
                        Err(failure) => return (Err(failure), batch_end),
                    },
                    None => item.clone(),
                };
                // A DISTRIBUTED Map "starts a child workflow execution for each
                // item", so this item's iteration runs as an execution of its own:
                // its ARN is minted HERE, before the iteration, because the item's
                // states read it through `$$.Execution.Id` — and the context they
                // read is the CHILD's, not the parent's. An inline Map starts no
                // execution, so its iteration keeps the parent's context.
                let child = plan.distributed.then(|| {
                    let name = self.fresh_child_name();
                    let arn = super::super::execution_arn(
                        &self.region,
                        &format!("{}/{}", self.machine_name, plan.label),
                        &name,
                    );
                    (name, arn)
                });
                let enclosing_context = child.as_ref().map(|(name, arn)| {
                    let child_context =
                        self.child_context(arn, name, &item_input, batch_start);
                    std::mem::replace(&mut self.base_context, child_context)
                });
                // The child's own `input` is the document this iteration was handed,
                // kept before the walk takes it: "the string that contains the JSON
                // input data" of that execution.
                let child_input = child.as_ref().map(|_| item_input.to_string());
                // One iteration per item: it is STARTED here, carrying the index of
                // the item it was handed, and records its own outcome below — which is
                // how a caller observes the fan-out, and how many items ran at once.
                let from = self.steps.len();
                self.push_detailed(
                    super::super::EVENT_MAP_ITERATION_STARTED,
                    plan.name,
                    batch_start,
                    iteration_details(index),
                );
                units.push(Unit {
                    from,
                    map_runs_from: self.map_runs.len(),
                    aborted: Some(Step {
                        event_type: super::super::EVENT_MAP_ITERATION_ABORTED,
                        state_name: Some(plan.name.to_string()),
                        after: Duration::ZERO,
                        terminal: false,
                        details: iteration_details(index),
                    }),
                });
                let branch = self.nested_walk(
                    plan.states,
                    plan.start_at,
                    item_input,
                    batch_start,
                    &scope,
                    true,
                );
                if let Some(enclosing) = enclosing_context {
                    self.base_context = enclosing;
                }
                batch_end = batch_end.max(branch.end);
                if let Some((name, arn)) = child {
                    // The child execution this item ran as, as it ended: an item that
                    // produced a result is an execution that SUCCEEDED with that
                    // result as its output, and one whose iteration failed is an
                    // execution that FAILED with that failure's own error and cause.
                    let (status, output, error, cause) = match &branch.outcome {
                        Ok(value) => (
                            super::super::EXECUTION_STATUS_SUCCEEDED,
                            Some(value.to_string()),
                            None,
                            None,
                        ),
                        Err(failure) => (
                            super::super::EXECUTION_STATUS_FAILED,
                            None,
                            failure.error.clone(),
                            failure.cause.clone(),
                        ),
                    };
                    tally.children.push(StartedChildExecution {
                        name,
                        arn,
                        input: child_input.unwrap_or_default(),
                        output,
                        error,
                        cause,
                        status,
                        started_at: batch_start,
                        finished_at: branch.end,
                        // One item per child, or the batch's own count for a state
                        // that declares an `ItemBatcher`: see
                        // [`StartedChildExecution::item_count`].
                        item_count: match plan.batched {
                            true => i32::try_from(super::super::item_batcher::item_count(item))
                                .unwrap_or(i32::MAX),
                            false => 1,
                        },
                    });
                }
                let outcome_event = match &branch.outcome {
                    Ok(_) => super::super::EVENT_MAP_ITERATION_SUCCEEDED,
                    Err(_) => super::super::EVENT_MAP_ITERATION_FAILED,
                };
                self.push_detailed(
                    outcome_event,
                    plan.name,
                    branch.end,
                    iteration_details(index),
                );
                match branch.outcome {
                    Ok(value) => {
                        // The array the iterations are building is a document this
                        // state will put out, so it is bounded as it grows rather
                        // than only once it is whole.
                        match measure_payload(&value, "a 'Map' iteration") {
                            Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                            Err(failure) => return (Err(failure), batch_end),
                        }
                        if results_bytes > MAX_PAYLOAD_BYTES {
                            return (
                                Err(named(
                                    DATA_LIMIT_ERROR,
                                    format!(
                                        "the 'Map' state's iterations produced at least \
                                         {results_bytes} bytes, more than the \
                                         {MAX_PAYLOAD_BYTES} a state's output may carry"
                                    ),
                                )),
                                batch_end,
                            );
                        }
                        results.push(value);
                        tally.items.push(MapItemOutcome {
                            finished_at: branch.end,
                            failed: false,
                        });
                    }
                    Err(failure) => {
                        failures += 1;
                        tally.items.push(MapItemOutcome {
                            finished_at: branch.end,
                            failed: true,
                        });
                        if first_failure
                            .as_ref()
                            .is_none_or(|(_, when)| branch.end < *when)
                        {
                            first_failure = Some((failure, branch.end));
                        }
                        // A state that tolerates no failure has already failed, so the
                        // iterations running BESIDE this one in the same batch are
                        // stopped at the instant it failed, exactly as the items it had
                        // not started yet are never started
                        // ([`Interpreter::stopped_by_a_sibling`]).
                        if !tolerant {
                            let stop =
                                first_failure.as_ref().map_or(branch.end, |(_, when)| *when);
                            self.cancel_after = Some(
                                self.cancel_after.map_or(stop, |enclosing| enclosing.min(stop)),
                            );
                        }
                    }
                }
                index += 1;
            }
            at = batch_end;
            // A state that tolerates no failure has already failed, so the items it
            // had not started yet are never started.
            if failures > 0 && !tolerant {
                break;
            }
        }

        if failures == 0 {
            return (Ok(Value::Array(results)), at);
        }
        if super::super::within_tolerated_failures(
            i64::try_from(failures).unwrap_or(i64::MAX),
            i64::try_from(plan.items.len()).unwrap_or(i64::MAX),
            i64::try_from(plan.tolerated_count).unwrap_or(i64::MAX),
            plan.tolerated_percentage,
        ) {
            return (Ok(Value::Array(results)), at);
        }
        // A Distributed Map reports its own tolerated-failure error; an inline Map
        // has no threshold to exceed, so the iteration's own failure is the state's.
        if tolerant {
            return (
                Err(named(
                    TOLERATED_FAILURE_ERROR,
                    format!(
                        "{failures} of {} items failed, more than this 'Map' state tolerates",
                        plan.items.len()
                    ),
                )),
                at,
            );
        }
        let Some((failure, when)) = first_failure else {
            return (Err(runtime("a 'Map' iteration failed".to_string())), at);
        };
        self.abort_units(when, &units);
        (Err(failure), when)
    }

    /// Where the concurrent unit that PARKED carries on from, for a walk continuing
    /// an execution whose job has ended: the next concurrent state down, else the
    /// state that started the job itself.
    pub(super) fn resume_entry(&self) -> Option<(String, Value)> {
        if let Some(frame) = self.resume_frames.first() {
            return Some((frame.state_name.clone(), frame.state_input.clone()));
        }
        let reply = self.job_reply.as_ref()?;
        Some((
            reply.state_name.clone(),
            serde_json::from_str(&reply.state_input).unwrap_or(Value::Null),
        ))
    }

    /// Stop the branches or iterations of a state that has FAILED, at the instant
    /// `when` it failed.
    ///
    /// A state whose concurrent work has already failed does not go on running the
    /// rest of it: each unit is cut back to the events that were due at or before that
    /// instant, the state it was in when the failure reached it records that type's own
    /// aborted event, and the unit itself records the one its kind has. `units` is
    /// where each unit's events begin in this walk's list, in the order they were run,
    /// so they are cut from the last one back and no earlier unit's position moves.
    pub(super) fn abort_units(&mut self, when: Duration, units: &[Unit]) {
        for (position, unit) in units.iter().enumerate().rev() {
            // Every bound is clamped to the list as it stands: a later unit has
            // already been cut back by the time this one is, so the span this reads
            // is the one the list still holds.
            let until = units
                .get(position + 1)
                .map_or(self.steps.len(), |next| next.from)
                .min(self.steps.len());
            let from = unit.from.min(until);
            let Some(cut) = (from..until).find(|index| {
                self.steps
                    .get(*index)
                    .is_some_and(|step| step.after > when)
            }) else {
                continue;
            };
            // The states this unit was still inside when the failure reached it: each
            // one entered and not yet left, innermost last.
            let mut open: Vec<(&'static str, Option<String>)> = Vec::new();
            for step in self.steps.get(from..cut).unwrap_or_default() {
                if let Some(entered) = step.event_type.strip_suffix(STATE_ENTERED) {
                    open.push((entered, step.state_name.clone()));
                } else if let Some(left) = step.event_type.strip_suffix(STATE_EXITED) {
                    if let Some(index) = open.iter().rposition(|(name, _)| *name == left) {
                        open.remove(index);
                    }
                }
            }
            let mut stopped: Vec<Step> = Vec::new();
            for (entered, state_name) in open.iter().rev() {
                if let Some(event_type) = aborted_event(entered) {
                    stopped.push(Step {
                        event_type,
                        state_name: state_name.clone(),
                        after: when,
                        terminal: false,
                        details: super::super::EventDetails::default(),
                    });
                }
            }
            if let Some(aborted) = &unit.aborted {
                let mut aborted = aborted.clone();
                aborted.after = when;
                stopped.push(aborted);
            }
            self.steps.splice(cut..until, stopped);
        }
        // A Map Run one of THESE units only started after the state had already failed
        // never started: the state it belongs to was not entered. The list also holds
        // the Map Runs of work that finished before these units began — an earlier
        // `Parallel` branch's, the walk's own — whose own instants this failure says
        // nothing about, so only what these units recorded is judged.
        let recorded_from = units
            .first()
            .map_or(self.map_runs.len(), |unit| unit.map_runs_from)
            .min(self.map_runs.len());
        let mut recorded = self.map_runs.split_off(recorded_from);
        recorded.retain(|(_, map_run)| map_run.started_at <= when);
        self.map_runs.append(&mut recorded);
    }

    // -----------------------------------------------------------------------
    // Retry and Catch
    // -----------------------------------------------------------------------

    /// Run `work`, re-running it while one of the state's `Retry` retriers matches
    /// what it failed with and has attempts left.
    ///
    /// The work is really RE-RUN, and the interval each retrier declares really
    /// lengthens the run, so a definition that retries reports an outcome that came
    /// due later than one that does not. The attempt the work is on is what the
    /// retried state reads through `$$.State.RetryCount`.
    pub(super) fn with_retries(
        &mut self,
        state: &Map<String, Value>,
        at: Duration,
        work: &mut dyn FnMut(&mut Self, Duration) -> Attempt,
    ) -> Attempt {
        let enclosing = self.retry_count;
        let answer = self.retry_loop(state, at, work);
        self.retry_count = enclosing;
        answer
    }

    /// [`Interpreter::with_retries`]' loop, which leaves the attempt count for its
    /// caller to restore.
    fn retry_loop(
        &mut self,
        state: &Map<String, Value>,
        at: Duration,
        work: &mut dyn FnMut(&mut Self, Duration) -> Attempt,
    ) -> Attempt {
        let mut at = at;
        let mut attempts: Vec<u64> = state
            .get("Retry")
            .and_then(Value::as_array)
            .map(|retriers| vec![0; retriers.len()])
            .unwrap_or_default();
        // A walk CONTINUING from an answer to a task this state SUSPENDED at takes up
        // the retrier budget that task carried: the budget a `Retry` gives is the
        // STATE's, so `MaxAttempts` counts every attempt the execution has made at it
        // rather than restarting at each answer — which is what makes such a state
        // fail once its retriers are spent instead of retrying for ever. Set by
        // [`Interpreter::task_state`] for exactly the state the answer resumes.
        if let Some(spent) = self.resumed_attempts.take() {
            for (slot, taken) in attempts.iter_mut().zip(spent) {
                *slot = taken;
            }
        }
        // `$$.State.RetryCount` is how many attempts this state has already made,
        // which for a resumed one is what those retriers have spent between them.
        self.retry_count = attempts.iter().copied().fold(0, u64::saturating_add);
        loop {
            // What the state has spent so far, for a task this attempt may SUSPEND at:
            // the record it leaves carries it, so the answer's own walk can seed this
            // loop from it.
            self.attempts_spent = attempts.clone();
            let (outcome, end) = work(self, at);
            at = end;
            let Err(failure) = outcome else {
                return (outcome, at);
            };
            let Some(retriers) = state.get("Retry").and_then(Value::as_array) else {
                return (Err(failure), at);
            };
            let mut retried = false;
            for (index, retrier) in retriers.iter().enumerate() {
                let Some(retrier) = retrier.as_object() else {
                    continue;
                };
                if !errors_match(retrier, failure.name()) {
                    continue;
                }
                let max_attempts = retrier
                    .get("MaxAttempts")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
                let taken = attempts.get(index).copied().unwrap_or(0);
                if taken >= max_attempts {
                    break;
                }
                // The delay is already clamped finite and inside
                // [`MAX_WAIT_SECONDS`] by [`retry_delay_seconds`], whatever a
                // definition writes in `IntervalSeconds`, `BackoffRate` and
                // `MaxDelaySeconds`. The conversion and the accumulation are
                // nevertheless written so NEITHER can panic: a caller-sized figure
                // must not be able to take the process down, so a delay that somehow
                // left that range waits no time at all and the offset saturates
                // rather than overflowing.
                let delay = Duration::try_from_secs_f64(retry_delay_seconds(retrier, taken))
                    .unwrap_or(Duration::ZERO);
                at = at.saturating_add(delay);
                if let Some(slot) = attempts.get_mut(index) {
                    *slot = taken + 1;
                }
                retried = true;
                break;
            }
            if !retried {
                return (Err(failure), at);
            }
            // A retry re-runs the work, so it spends from the same step budget: a
            // definition cannot ask for an unbounded number of attempts.
            if self.budget == 0 {
                return (Err(failure), at);
            }
            self.budget -= 1;
            self.retry_count += 1;
        }
    }

    /// The state the first matching `Catch` catcher names, or the failure itself when
    /// none does.
    ///
    /// `left` is the event the state records when a catcher takes over, so a caught
    /// state's history pairs the event it was entered with.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn catch_or_fail(
        &mut self,
        caught: (&str, &'static str),
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: &Value,
        failure: Failure,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let (name, left) = caught;
        let catchers = state
            .get("Catch")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for catcher in catchers {
            let Some(catcher) = catcher.as_object() else {
                continue;
            };
            if !errors_match(catcher, failure.name()) {
                continue;
            }
            let Some(next) = catcher.get("Next").and_then(Value::as_str) else {
                return Err(
                    runtime("a 'Catch' catcher must carry a string 'Next'".to_string()).at(at),
                );
            };
            // "When a state reports an error and it matches a catcher, the state's
            // output is the error output", placed through the catcher's own
            // `ResultPath` — or, in JSONata, through the catcher's own `Output` and
            // `Assign`, which are the two places `$states.errorOutput` is in scope.
            let error_output = failure.payload();
            let value = match language {
                JSONATA => self
                    .jsonata_output(catcher, effective, None, Some(&error_output), context)
                    .map_err(|failure| failure.at(at))?,
                _ => self
                    .place_result(catcher, raw, error_output, context, true)
                    .map_err(|failure| failure.at(at))?,
            };
            self.push_exited(left, name, at, &value);
            return Ok(StateStep {
                value,
                at,
                next: Next::Named(next.to_string()),
            });
        }
        Err(failure.at(at))
    }

    /// A numeric member of a state, from the literal member or from its `*Path` form.
    pub(super) fn number_member(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        input: &Value,
        context: &Value,
        default: f64,
    ) -> Result<f64, Failure> {
        let path_member = format!("{member}Path");
        let value = match (state.get(member), state.get(&path_member)) {
            (Some(value), _) => self
                .literal(language, value, input, context)?
                .unwrap_or(Value::Null),
            (None, Some(Value::String(path))) => expression(path, input, context)?,
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'{path_member}' must be a JSONPath string, not {other}"
                )));
            }
            (None, None) => return Ok(default),
        };
        value.as_f64().filter(|number| *number >= 0.0).ok_or_else(|| {
            runtime(format!(
                "'{member}' must be a non-negative number, not {value}"
            ))
        })
    }
}
