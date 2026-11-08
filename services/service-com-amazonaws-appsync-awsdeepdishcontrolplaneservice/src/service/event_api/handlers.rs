// SPDX-License-Identifier: CC0-1.0

//! The channel namespace's EVENT HANDLERS, run: `ChannelNamespace$codeHandlers` is "the
//! event handler functions that run custom business logic to process published events and
//! subscribe requests", so a publish to a namespace that carries code is answered by what
//! that code returns and by nothing else.
//!
//! The code is APPSYNC_JS — an ES module exporting `onPublish(ctx)` and/or
//! `onSubscribe(ctx)` — and it is RUN, by the QuickJS engine embedded in this process
//! (`rquickjs`), never interpreted by anything written here: no pattern is matched against
//! the handler's source and no answer is synthesised from it.
//!
//! # What the code can reach
//!
//! A fresh engine per run, with no host function installed on it beyond the one the
//! handler's own logging writes through, and a module resolver that answers with ONE
//! module — the `util` helpers AppSync's own handlers import ([`UTILS_MODULE`]) — and with
//! nothing else, so an `import` of any other specifier fails to resolve rather than
//! reaching a host path. Plain QuickJS exposes no filesystem, process, environment,
//! network or thread API, and this module adds none.
//!
//! # What bounds it
//!
//! Every bound lives in host code the handler cannot reach, and each is checked by the
//! engine's own interrupt handler as it works:
//!
//! * a WALL CLOCK ([`RUN_WALL_CLOCK`]), so a handler that never returns is stopped;
//! * a STEP budget ([`RUN_ENGINE_CHECKS`]), so a tight loop that makes no progress is
//!   stopped even where the clock has not run out;
//! * a MEMORY ceiling ([`RUN_MEMORY_BYTES`]) and a STACK ceiling ([`RUN_STACK_BYTES`]),
//!   both the engine's own, so an allocation or a recursion past them ends the run;
//! * a RESULT ceiling ([`RUN_RESULT_BYTES`]) on what the handler hands back, so a handler
//!   returning an unbounded document cannot be read into this process.
//!
//! Which bound was breached is LATCHED in host state ([`Bounds`]) rather than read back
//! from the engine, so a handler that catches the interrupt's exception cannot make the
//! run report success: once the latch is set the run's answer is the breach.
//!
//! The run happens on a blocking thread ([`evaluate`]), never on the async executor that
//! is serving the publish.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::rquickjs::function::Func;
use ::rquickjs::loader::{ImportAttributes, Loader, Resolver};
use ::rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Runtime, Value};
use ::serde_json::Value as Json;

/// How long one handler may run before it is stopped.
///
/// A bound of this emulator's own, not a documented AppSync timeout: it exists so one
/// publish cannot hold a connection — or a thread — for longer than a publish takes.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(2);

/// How many engine checks one handler may spend. The engine asks the interrupt handler as
/// it works, so a loop that allocates nothing and calls nothing still runs the count down
/// and is stopped by it.
const RUN_ENGINE_CHECKS: u64 = 20_000_000;

/// The memory ceiling one run's engine is built with.
const RUN_MEMORY_BYTES: usize = 32 * 1024 * 1024;

/// The stack ceiling one run's engine is built with, which is what ends a runaway
/// recursion.
///
/// Well below the stack of the blocking thread the run happens on (`tokio`'s own, which is
/// measured in megabytes), so a recursion past this raises INSIDE the engine and is
/// reported as a failed run, rather than overflowing the host thread.
const RUN_STACK_BYTES: usize = 512 * 1024;

/// The largest document a handler may hand back, in bytes.
const RUN_RESULT_BYTES: usize = 1024 * 1024;

/// The ONE module specifier a handler may import, which is where AppSync's own handlers
/// take `util` from.
pub const UTILS_MODULE: &str = "@aws-appsync/utils";

/// The `util` helpers [`UTILS_MODULE`] exports, as the module's own source.
///
/// Only the ones whose effect is the handler's OWN control flow are here: `util.error` and
/// `util.unauthorized` RAISE, which is how a handler refuses a publish or a subscribe, and
/// `util.log` writes through the one host function this engine carries. Nothing else is
/// offered, so a handler reaching for a helper this emulator does not implement fails
/// loudly rather than silently doing nothing.
const UTILS_MODULE_SOURCE: &str = r#"
const raise = function (message, errorType, data, errorInfo) {
    const error = new Error(message === undefined ? "" : String(message));
    if (errorType !== undefined && errorType !== null) {
        error.name = String(errorType);
    }
    error.data = data;
    error.errorInfo = errorInfo;
    throw error;
};
export const util = {
    error: raise,
    appendError: raise,
    unauthorized: function () {
        const error = new Error("UnauthorizedException");
        error.name = "UnauthorizedException";
        throw error;
    },
    log: {
        info: function () { __appsyncLog("INFO", Array.prototype.join.call(arguments, " ")); },
        error: function () { __appsyncLog("ERROR", Array.prototype.join.call(arguments, " ")); },
    },
};
export default { util };
"#;

/// The name of the handler a publish runs.
pub const ON_PUBLISH: &str = "onPublish";

/// The name of the handler a subscribe runs.
pub const ON_SUBSCRIBE: &str = "onSubscribe";

/// What running one handler came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerRun {
    /// The module carries no handler of that name, so the request is served as a namespace
    /// with no code at all serves it.
    NotDeclared,
    /// The handler returned, with the document it returned (`null` for a handler that
    /// returned nothing).
    Returned(Json),
    /// The handler did not return: it raised, the module did not compile or load, or one of
    /// this module's bounds stopped it. The text is what the engine (or the bound) said.
    Failed(String),
}

/// Run `function` of `code` over `context`, on a blocking thread.
///
/// The engine, the bounds and the whole run live inside that thread, so nothing of the
/// engine crosses onto the executor serving the publish. A thread that cannot be joined is
/// reported as a failed run rather than taken for a handler that returned nothing.
pub async fn evaluate(code: String, function: &'static str, context: Json) -> HandlerRun {
    match ::tokio::task::spawn_blocking(move || run(&code, function, &context)).await {
        Ok(answer) => answer,
        Err(error) => HandlerRun::Failed(format!("the handler could not be run: {error}")),
    }
}

/// Run `function` of `code` over `context` on THIS thread, under every bound this module
/// declares.
fn run(code: &str, function: &str, context: &Json) -> HandlerRun {
    let Ok(context_json) = ::serde_json::to_string(context) else {
        return HandlerRun::Failed("the handler's context could not be rendered".to_string());
    };
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => return HandlerRun::Failed(format!("the engine did not start: {error}")),
    };
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    runtime.set_loader(UtilsResolver, UtilsLoader);
    let bounds = Arc::new(Bounds::new());
    let watched = Arc::clone(&bounds);
    // The only thing that can stop a handler with no exit: a timeout cannot preempt a loop
    // running on this thread, but the engine asks this handler as it works.
    runtime.set_interrupt_handler(Some(Box::new(move || watched.spent())));
    let context_handle = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => return HandlerRun::Failed(format!("the engine did not start: {error}")),
    };
    let answer = context_handle.with(|ctx| evaluate_in(&ctx, code, function, &context_json));
    // A bound that was breached is the run's answer, whatever the handler then said or
    // returned: the latch is host state the handler cannot reach.
    if let Some(breach) = bounds.breach() {
        return HandlerRun::Failed(breach);
    }
    answer
}

/// Compile the module, call `function` and read what it returned, inside one engine
/// context.
fn evaluate_in(ctx: &Ctx<'_>, code: &str, function: &str, context_json: &str) -> HandlerRun {
    if let Err(error) = install_log(ctx) {
        return HandlerRun::Failed(error);
    }
    let declared = match Module::declare(ctx.clone(), HANDLER_MODULE_NAME, code.to_string()) {
        Ok(declared) => declared,
        Err(error) => return HandlerRun::Failed(format!("the handler code does not compile: {error}")),
    };
    let evaluated = match declared.eval().catch(ctx) {
        Ok((evaluated, _promise)) => evaluated,
        Err(caught) => return HandlerRun::Failed(format!("the handler code does not run: {caught}")),
    };
    let Ok(handler) = evaluated.get::<_, Function<'_>>(function) else {
        return HandlerRun::NotDeclared;
    };
    let argument = match ctx.json_parse(context_json.to_string()) {
        Ok(argument) => argument,
        Err(error) => {
            return HandlerRun::Failed(format!("the handler's context could not be read: {error}"))
        }
    };
    let returned = match handler.call::<_, Value<'_>>((argument,)).catch(ctx) {
        Ok(returned) => returned,
        Err(caught) => return HandlerRun::Failed(caught.to_string()),
    };
    read_returned(ctx, &returned)
}

/// What the handler returned, as a document: `null` for a handler that returned nothing,
/// and a failure for one whose value cannot be rendered or is larger than
/// [`RUN_RESULT_BYTES`].
fn read_returned<'js>(ctx: &Ctx<'js>, returned: &Value<'js>) -> HandlerRun {
    let rendered = match ctx.json_stringify(returned.clone()) {
        Ok(Some(rendered)) => rendered,
        // `undefined` stringifies to nothing at all, which is a handler that returned
        // nothing.
        Ok(None) => return HandlerRun::Returned(Json::Null),
        Err(error) => {
            return HandlerRun::Failed(format!("the handler's answer could not be read: {error}"))
        }
    };
    let Ok(text) = rendered.to_string() else {
        return HandlerRun::Failed("the handler's answer could not be read".to_string());
    };
    if text.len() > RUN_RESULT_BYTES {
        return HandlerRun::Failed(format!(
            "the handler returned more than {RUN_RESULT_BYTES} bytes"
        ));
    }
    match ::serde_json::from_str(&text) {
        Ok(document) => HandlerRun::Returned(document),
        Err(error) => HandlerRun::Failed(format!("the handler's answer is not a document: {error}")),
    }
}

/// The unit name the handler's module is compiled under, so every frame of a stack taken
/// inside it names the handler rather than a host path.
const HANDLER_MODULE_NAME: &str = "channel-namespace-handlers";

/// Install the ONE host function the engine carries: what `util.log` writes through.
///
/// The lines go to this process's own log at debug level. Nothing else of the host is
/// reachable from the engine.
fn install_log(ctx: &Ctx<'_>) -> Result<(), String> {
    ctx.globals()
        .set(
            "__appsyncLog",
            Func::from(|level: String, message: String| {
                ::tracing::debug!(%level, %message, "a channel namespace handler logged");
            }),
        )
        .map_err(|error| format!("the handler log could not be installed: {error}"))
}

/// The run's budget, as the engine's interrupt handler spends it, and the latch that says
/// which bound ended the run.
#[derive(Debug)]
struct Bounds {
    /// When the run must be over.
    deadline: Instant,
    /// How many engine checks are left.
    checks: AtomicU64,
    /// Set once the clock ran out.
    timed_out: AtomicBool,
    /// Set once the checks ran out.
    steps_spent: AtomicBool,
}

impl Bounds {
    /// A budget starting now.
    fn new() -> Self {
        Self {
            deadline: Instant::now() + RUN_WALL_CLOCK,
            checks: AtomicU64::new(RUN_ENGINE_CHECKS),
            timed_out: AtomicBool::new(false),
            steps_spent: AtomicBool::new(false),
        }
    }

    /// Whether the run is over, asked by the engine as it works: the clock, then the step
    /// budget. Each answer latches, so the breach survives a handler that catches the
    /// exception the engine raises.
    fn spent(&self) -> bool {
        if Instant::now() >= self.deadline {
            self.timed_out.store(true, Ordering::Relaxed);
            return true;
        }
        if self.checks.fetch_sub(1, Ordering::Relaxed) <= 1 {
            self.steps_spent.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// What the breached bound is reported as, or `None` when none was breached.
    fn breach(&self) -> Option<String> {
        if self.timed_out.load(Ordering::Relaxed) {
            return Some(format!(
                "the handler was stopped after {} seconds",
                RUN_WALL_CLOCK.as_secs()
            ));
        }
        if self.steps_spent.load(Ordering::Relaxed) {
            return Some(format!(
                "the handler was stopped after {RUN_ENGINE_CHECKS} engine steps"
            ));
        }
        None
    }
}

/// How an `import` specifier is resolved: to [`UTILS_MODULE`] and to nothing else, so no
/// host path and no other module is reachable from a handler.
#[derive(Debug, Clone, Copy)]
struct UtilsResolver;

impl Resolver for UtilsResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> ::rquickjs::Result<String> {
        if name == UTILS_MODULE {
            return Ok(UTILS_MODULE.to_string());
        }
        Err(::rquickjs::Error::new_resolving(
            base.to_string(),
            name.to_string(),
        ))
    }
}

/// How the one resolvable module is loaded: from the source this module carries
/// ([`UTILS_MODULE_SOURCE`]). No path is opened for it.
#[derive(Debug, Clone, Copy)]
struct UtilsLoader;

impl Loader for UtilsLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> ::rquickjs::Result<Module<'js>> {
        if path != UTILS_MODULE {
            return Err(::rquickjs::Error::new_loading(path.to_string()));
        }
        Module::declare(ctx.clone(), path.to_string(), UTILS_MODULE_SOURCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::serde_json::json;

    /// Run `code`'s `onPublish` over one context, on this thread.
    fn publish(code: &str, context: Json) -> HandlerRun {
        run(code, ON_PUBLISH, &context)
    }

    #[test]
    fn a_handler_answers_with_what_its_own_code_returns() {
        let context = json!({
            "events": [
                {"id": "1", "payload": {"keep": true}},
                {"id": "2", "payload": {"keep": false}},
            ]
        });
        // The rows span what the running generalises over: a filter, a transform, a
        // handler that returns nothing, a module that exports another handler only, a
        // handler that raises, code that does not compile, and an import of a module this
        // engine does not serve.
        let cases: &[(&str, &str, HandlerRun)] = &[
            (
                "a filter over the payload",
                "export function onPublish(ctx) { return ctx.events.filter(e => e.payload.keep) }",
                HandlerRun::Returned(json!([{"id": "1", "payload": {"keep": true}}])),
            ),
            (
                "a transform of every event",
                "export function onPublish(ctx) { return ctx.events.map(e => ({id: e.id, \
                 payload: {seen: e.payload.keep}})) }",
                HandlerRun::Returned(json!([
                    {"id": "1", "payload": {"seen": true}},
                    {"id": "2", "payload": {"seen": false}},
                ])),
            ),
            (
                "a handler that returns nothing",
                "export function onPublish(ctx) { }",
                HandlerRun::Returned(Json::Null),
            ),
            (
                "a module that declares another handler only",
                "export function onSubscribe(ctx) { return null }",
                HandlerRun::NotDeclared,
            ),
            (
                "the util module this engine serves",
                "import { util } from '@aws-appsync/utils';\n\
                 export function onPublish(ctx) { util.log.info('seen'); return [] }",
                HandlerRun::Returned(json!([])),
            ),
        ];
        for (case, code, expected) in cases {
            let answer = publish(code, context.clone());
            assert_eq!(
                answer, *expected,
                "case {case:?}: expected {expected:?}, got {answer:?}"
            );
        }
    }

    #[test]
    fn a_handler_that_does_not_return_is_a_failed_run() {
        // The rows span the ways a run fails: the handler raises through `util.error`, it
        // throws on its own, the code does not compile, and it imports a module no
        // resolver of this engine answers.
        let cases: &[(&str, &str)] = &[
            (
                "util.error",
                "import { util } from '@aws-appsync/utils';\n\
                 export function onPublish(ctx) { util.error('no', 'Refused') }",
            ),
            (
                "a thrown error",
                "export function onPublish(ctx) { throw new Error('no') }",
            ),
            (
                "code that does not compile",
                "export function onPublish(ctx) { return (((( }",
            ),
            (
                "an import of another module",
                "import fs from 'node:fs';\nexport function onPublish(ctx) { return ctx.events }",
            ),
            (
                "a recursion with no base case",
                "export function onPublish(ctx) { function down(n) { return down(n + 1) } \
                 return down(0) }",
            ),
        ];
        for (case, code) in cases {
            let answer = publish(code, json!({"events": []}));
            assert!(
                matches!(answer, HandlerRun::Failed(_)),
                "case {case:?}: expected a failed run, got {answer:?}"
            );
        }
    }

    #[test]
    fn a_handler_that_does_not_end_is_stopped_by_a_bound() {
        // A loop that makes no progress: it allocates nothing and calls nothing, so the
        // step budget and the wall clock are the only things that can end it.
        let answer = publish(
            "export function onPublish(ctx) { while (true) {} }",
            json!({"events": []}),
        );
        match answer {
            HandlerRun::Failed(reason) => assert!(
                reason.contains("stopped"),
                "expected the run to be reported as stopped, got {reason:?}"
            ),
            other => panic!("expected a stopped run, got {other:?}"),
        }
    }

    #[test]
    fn a_handler_cannot_switch_off_the_bound_that_stops_it() {
        // The handler catches the exception the engine raises and keeps going: the latch
        // is host state, so the run is still reported as stopped rather than as the value
        // the handler returns.
        let answer = publish(
            "export function onPublish(ctx) {\n\
               while (true) { try { } catch (error) { } }\n\
             }",
            json!({"events": []}),
        );
        assert!(
            matches!(answer, HandlerRun::Failed(ref reason) if reason.contains("stopped")),
            "expected a stopped run whatever the handler caught, got {answer:?}"
        );
    }
}
