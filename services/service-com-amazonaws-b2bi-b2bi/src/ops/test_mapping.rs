// SPDX-License-Identifier: CC0-1.0

//! @op TestMapping
//! Runs the caller's `mappingTemplate` over `inputFileContent` through a REAL JSONata
//! interpreter (the `jsonata-rs` crate) and answers the serialized result, so
//! `mappedFileContent` is computed from the request's own document rather than echoed
//! or synthesized.
//!
//! `TestMapping` is a dry run of a transformer's mapping step: it owns no resource and
//! mints nothing, so it neither reads nor writes `crate::state` — a request changes
//! nothing a later caller sees, and two identical requests answer identically unless
//! the template itself asks for a clock or a random value (`$now`, `$millis`,
//! `$random`, `$uuid`).
//!
//! ## Running caller-supplied code
//!
//! A `mappingTemplate` is code the caller wrote, so every resource it can reach is
//! bounded BEFORE it is reached, and none of it runs on the async worker that serves the
//! request:
//!
//! * MEMORY is clamped ahead of every allocation: the evaluation's arena carries
//!   `set_allocation_limit(Some(`[`MAX_EVALUATION_ARENA_BYTES`]`))`, which `bumpalo`
//!   checks before it asks the system for a chunk, and refuses by PANICKING, never by
//!   aborting — its `Bump::oom` is `panic!("out of memory")` and its
//!   `alloc::handle_alloc_error` is `panic!("encountered allocation error: …")`. The
//!   evaluation runs inside [`::std::panic::catch_unwind`], so that refusal unwinds (and
//!   frees the arena) and is answered as the modelled 400: a template cannot grow this
//!   process past the ceiling, and cannot kill it either;
//! * nearly the whole language is evaluated — selecting, filtering, aggregating, slicing,
//!   reshaping, concatenating, serializing and replacing — because a mapping template
//!   that cannot concatenate or serialize a value cannot express what an EDI transform
//!   does with its output. Every one of those builtins asks the ARENA for its result
//!   buffer, so the clamp above precedes it, which is MEASURED rather than assumed (see
//!   [`unbounded_construct`] and `tests/all/test_mapping.rs`);
//! * the three constructs whose ONE step has no bound this handler can place ahead of it
//!   — a regular-expression literal and `$distinct`, whose single step has no time bound
//!   and cannot be interrupted, and `$pad`, whose buffer is a global-heap `String` sized
//!   by a number in the template rather than by any value — are refused on the engine's
//!   own parse, before the template is evaluated at all ([`unbounded_construct`]) and
//!   with the 501 that names the capability this emulator is missing;
//! * how deeply the PARSE may recurse needs no trust in a scan of the caller's text: the
//!   evaluation thread's stack ([`EVALUATION_STACK_BYTES`]) is sized for one parser frame
//!   per BYTE of the longest template this emulator accepts, measured, because a parser
//!   stack overflow is the one failure `catch_unwind` cannot answer — it aborts.
//!   [`max_template_nesting`] refuses the ordinary deep-nesting case earlier and more
//!   cheaply ([`MAX_TEMPLATE_NESTING`]), but nothing unsurvivable rests on it;
//! * it runs on a thread of its own ([`EVALUATION_STACK_BYTES`]), so a long evaluation
//!   cannot hold a worker that health checks, other requests and shutdown need, and the
//!   worst-case parser recursion has a stack sized for it;
//! * at most [`MAX_CONCURRENT_EVALUATIONS`] run at once ([`EVALUATION_PERMITS`]), and a
//!   request that waits [`PERMIT_WAIT`] for a slot is throttled rather than queued. The
//!   permit is held by the REQUEST, so a slot cannot be retired for the life of the
//!   process by an evaluation that outran its caller; what bounds the threads such an
//!   evaluation leaves behind is [`MAX_LIVE_EVALUATIONS`], counted in
//!   [`LIVE_EVALUATIONS`];
//! * the interpreter's own time box ([`EVALUATION_TIME_LIMIT_MS`]) and depth box
//!   ([`MAX_EVALUATION_DEPTH`]) end a non-terminating or deeply recursive template, and
//!   [`EVALUATION_DEADLINE`] is the runtime-level backstop that frees the caller;
//! * the template ([`MAX_TEMPLATE_BYTES`], 16 KiB against the modelled 350 000) and the
//!   input document ([`MAX_DOCUMENT_BYTES`], 1 MiB against the modelled 5 000 000) are
//!   this emulator's own, smaller ceilings, so a request within the modelled lengths can
//!   still be declined here — with the 501 that names the ceiling
//!   ([`ceiling_not_implemented`]), since the service would have mapped it; the answered
//!   document ([`MAX_MAPPED_FILE_BYTES`]) is capped on the VALUE, before it is
//!   serialized, and the member that carries its text is that text escaped, so at most
//!   twice the capped document plus its two quotes.
//!
//! There is no host escape to disable: JSONata's function set is fixed by the
//! interpreter, and its whole list (`jsonata-rs`'s `bind_native!` bindings: `$abs` …
//! `$zip`, plus `$now`/`$millis`/`$random`/`$uuid`) holds no filesystem, process,
//! socket, environment, import or eval builtin. This handler gives the evaluation nothing
//! but the two strings from the request — no state, no credentials, no request metadata,
//! and no I/O of its own.
//!
//! ## Why one uninterruptible step is bounded
//!
//! A `std::thread` cannot be aborted, so [`EVALUATION_DEADLINE`] frees the CALLER while
//! the evaluation ends at its own next budget check. The interpreter checks that budget
//! on entry to and exit from every evaluation step, so the longest stretch nothing can
//! interrupt is ONE step, and a step is bounded as follows:
//!
//! * an interpreter node is itself a step, so recursion, `$map`, `$reduce`, `$sort`'s
//!   comparator and every other loop a template can write IS interrupted between
//!   iterations;
//! * each remaining builtin computes in time and space proportional to values already in
//!   the arena, which the allocation limit clamps — and the range operator, the one
//!   construct left whose size comes from a number rather than from data, is capped by
//!   the engine itself at 1e7 elements (`jsonata-rs` answers `D2014` past that), inside
//!   the same limit;
//! * a string-building builtin (`$string`, the `&` operator, `$join`, `$replace`) asks the
//!   ARENA for the buffer it writes its result into, so the allocation limit declines an
//!   over-ceiling buffer before the system is asked for it. That is MEASURED, not
//!   assumed: a template that aliases one 100 KB member a thousand times and joins,
//!   concatenates or serializes it is refused with the arena ceiling named in the message
//!   (the engine asks the arena for the whole 100 000 000-byte buffer and `bumpalo`
//!   declines the layout), while the same shape at a hundred aliases is refused by the
//!   answer ceiling — `tests/all/test_mapping.rs` sweeps the alias count and asserts
//!   WHICH bound answered, so an engine upgrade that moved a buffer off the arena fails
//!   the build;
//! * the two steps with no time bound at all — a regular-expression literal, backtracked
//!   by `regress` without a step budget, and `$distinct`, whose native pass is quadratic
//!   in the member count — are refused on the parse ([`unbounded_construct`]) rather than
//!   left to the backstop, because a `std::thread` cannot be cancelled: such a step would
//!   hold one of [`MAX_LIVE_EVALUATIONS`] threads, and a core, for the life of the
//!   process;
//! * the parse is outside the interpreter's budget ([`::jsonata_rs::JsonAta::new`] takes
//!   none), which is why [`MAX_TEMPLATE_BYTES`] bounds it instead, on a stack sized for
//!   its worst case;
//! * while an evaluation runs it keeps its THREAD but not its permit: the permit belongs
//!   to the request, so an evaluation that outran [`EVALUATION_DEADLINE`] cannot retire a
//!   slot permanently, and [`MAX_LIVE_EVALUATIONS`] bounds how many such threads can
//!   exist before new requests are throttled — so a flood cannot pile threads up without
//!   limit and a wedged evaluation cannot disable the operation for good.
//!
//! ## What the engine evaluates
//!
//! JSONata as `jsonata-rs` implements it, less the three constructs named below. Read off
//! the engine's own bindings, each one answers a number or a boolean (`$count`, `$sum`,
//! `$max`, `$min`, `$abs`, `$floor`, `$ceil`, `$round`, `$sqrt`, `$power`, `$number`,
//! `$boolean`, `$not`, `$exists`, `$length`, `$millis`, `$toMillis`, `$random`), or a
//! fresh ARENA value the allocation limit charges (`$map`, `$filter`, `$reduce`, `$each`,
//! `$single`, `$sort`, `$reverse`, `$append`, `$zip`, `$merge`, `$keys`, `$lookup`,
//! `$uuid`, `$now`, `$fromMillis`, `$error`, `$assert`), or a string whose buffer the
//! arena also allocates (`$uppercase`, `$lowercase`, `$trim`, `$substring`,
//! `$substringBefore`, `$substringAfter`, `$split`, `$contains`, `$replace`,
//! `$base64encode`, `$base64decode`, `$join`, `$string`, the `&` operator) — so the
//! allocation limit precedes every one of them.
//!
//! The three it refuses, each on the engine's own parse and with the 501 that names it
//! ([`unbounded_construct`]): a regular-expression literal and `$distinct`, whose single
//! step has no time bound and cannot be interrupted, and `$pad`, whose buffer is a
//! global-heap `String` sized by a number in the template rather than by any value, so no
//! ceiling here precedes it. Everything a mapping template needs to BUILD its output —
//! concatenation, `$join`, `$string`, `$replace` with a string pattern — is evaluated.
//!
//! ## Fidelity notes
//!
//! Some answers here are this emulator's, not the real service's, and a test written
//! against them is not testing AWS:
//!
//! * real `TestMapping` maps an XSLT template and an XML document too. This emulator has
//!   no XSLT engine, so BOTH an XSLT `mappingTemplate` and a JSONata template over a
//!   document declared `fileFormat XML` are answered with the 501 that names that missing
//!   engine — neither of which AWS answers for those inputs. What AWS does there is map
//!   the document; nothing else about that path is asserted here;
//! * `mappedFileContent` is modelled as a plain `String` and carries the mapped DOCUMENT,
//!   so it is that document rendered to TEXT: pretty-printed with two-space indentation
//!   (see [`mapped_document_text`]) and then JSON-string-encoded into the member
//!   ([`mapped_file_member`]), so a caller parses the member once for the mapped file's
//!   text and parses that text for the document. Every object's members are in SORTED key
//!   order. That last part is a known divergence, not a rule: the real service writes a
//!   mapped object's
//!   members in the order its mapping constructed them, and nothing here can reproduce that
//!   order — the interpreter holds an object's members in a hash map, so its own order
//!   varies between evaluations, and it exposes no typed view of the parsed template to
//!   read the construction order from. A test written to run against AWS as well should
//!   compare `mappedFileContent` as PARSED JSON rather than as a byte string;
//! * a template using a regular-expression literal, `$distinct` or `$pad` is answered
//!   with the 501 that names the construct this emulator does not evaluate instead of
//!   mapped ([`unbounded_construct`] gives the bound each one lacks, and the measured
//!   evidence that every other construct's buffer is arena-clamped). Real `TestMapping`
//!   evaluates all three, so a mapping that matches a pattern, de-duplicates a list or
//!   pads a fixed-width field is UNANSWERABLE here, and that is the largest thing a suite
//!   written against this emulator cannot conclude about AWS. `$match` and the pattern
//!   forms of `$split`/`$contains`/`$replace` need a literal, so they are covered by the
//!   same refusal, while those builtins over plain strings — and concatenation, `$join`,
//!   `$string` and `$replace` with a string pattern, which is what a mapping builds its
//!   output with — are evaluated. The refusal also OVER-refuses in two documented shapes:
//!   a template whose own variable is merely NAMED after one of those builtins
//!   (`($pad := 1; $pad)`), and a string literal containing the text `Regex(`;
//! * `mappedFileContent`'s own member documentation in the model reads "Returns a string
//!   for the mapping that can be used to identify the mapping. Similar to a fingerprint",
//!   which is a doc artefact rather than the operation's behaviour: the operation-level
//!   documentation ("Maps the input file according to the provided template file") and the
//!   recorded round-trip units both say the member carries the MAPPED DOCUMENT, which is
//!   what this handler answers. Nothing here should be "fixed" back to a fingerprint;
//! * the request IS authorized, like every other operation of this service whose model
//!   declares `AccessDeniedException` (403): the caller's IAM identity policies decide it
//!   ([`crate::state::authorize`], against [`crate::state::ANY_RESOURCE`] because a
//!   mapping dry run addresses no resource ARN), so an explicit `Deny` on
//!   `b2bi:TestMapping` refuses the call and nothing is evaluated;
//! * a mapped answer carries HTTP 200. The model puts an `@http` code of 201 on this
//!   operation, but B2BI speaks awsJson1_0, an RPC protocol on which the `@http` bindings
//!   are inert — the model reports them so
//!   (`smithy_query(query="operation",operation="TestMapping")`: `httpBindingsInert`) —
//!   and a success on that protocol is 200, which is what every operation of this service
//!   answers;
//! * a zero-length `inputFileContent` is mapped as "no input document" (the template is
//!   evaluated with no context) rather than refused. The model allows `len 0..5000000`,
//!   but the real service downloads a file and passes its contents in, so what it answers
//!   for an empty one is not something this emulator knows; the mapped answer is this
//!   emulator's choice, and a test that asserts success on an empty input is asserting
//!   that choice;
//! * every ceiling in "Running caller-supplied code" above is this emulator's own, and
//!   AWS documents no evaluation deadline, depth, memory, nesting or concurrency limit
//!   for `TestMapping`, so none of these answers is service behaviour. A request inside
//!   the modelled lengths that is past one of the three ceilings this emulator applies
//!   BEFORE it attempts the work — the template's size, the document's size, or the
//!   template's nesting — is answered with the 501 naming that ceiling
//!   ([`ceiling_not_implemented`]); a failure found while EVALUATING — the time box, the
//!   interpreter's own depth box, the arena ceiling, or a result past the answered
//!   document's size or nesting ceiling ([`fits_mapped_ceiling`]) — is the modelled 400,
//!   and a request that waits out the evaluation permit is the modelled 429.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `TestMapping::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `TestMapping`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json")
//! ```
//!
//! Maps the input file according to the provided template file. The API call downloads the
//! file contents from the Amazon S3 location, and passes the contents in as a string, to
//! the inputFileContent parameter.
//!
//! ## Input
//!
//! * `fileFormat` (FileFormat, one of: JSON | NOT_USED | XML) **required** — Specifies that
//!   the currently supported file formats for EDI transformations are JSON and XML.
//! * `inputFileContent` (TestMappingInputFileContent, len 0..5000000) **required** — Specify
//!   the contents of the EDI (electronic data interchange) XML or JSON file that is used as
//!   input for the transform.
//! * `mappingTemplate` (MappingTemplate, len 0..350000) **required** — Specifies the mapping
//!   template for the transformer. This template is used to map the parsed EDI file using
//!   JSONata or XSLT. This parameter is available for backwards compatibility. Use the
//!   Mapping data type instead.
//!
//! ## Output
//!
//! * `mappedFileContent` (String) **required** — Returns a string for the mapping that can be
//!   used to identify the mapping. Similar to a fingerprint
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — This exception is thrown when an error occurs in
//!   the Amazon Web Services B2B Data Interchange service.
//! * `ResourceNotFoundException` (HTTP 404) — Occurs when the requested resource does not
//!   exist, or cannot be found. In some cases, the resource exists in a region other than the
//!   region specified in the API call.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling: the data
//!   speed and rendering may be limited depending on various parameters and conditions.
//! * `ValidationException` (HTTP 400) — Occurs when a B2BI object cannot be validated against
//!   a request from another object. This exception can be thrown during standard EDI
//!   validation or when custom validation rules fail, such as when element length constraints
//!   are violated, invalid codes are used in code list validations, or required elements are
//!   missing based on configured element requirement rules.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TestMappingError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-b2bi-b2bi.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TestMappingError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.b2bi#TestMapping")]
#[derive(Debug)]
pub enum TestMappingErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::test_mapping::TestMappingError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TestMappingError = crate::scaffolded::Scaffolded<TestMappingErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TestMappingErrorModeled> for TestMappingError {
    fn from(modeled: TestMappingErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// This operation's name, as the `tracing` records and the modelled messages spell it.
const OPERATION: &str = "TestMapping";

/// The bounded JSONata evaluation of a caller-supplied mapping template, for the OTHER
/// operation of this service that evaluates one: the mapping a transformer job applies
/// (`super::start_transformer_job`).
///
/// It is [`map_document`] — THIS operation's own body — so a job's mapping runs under
/// exactly the containment a `TestMapping` request's does, and the two cannot drift: the
/// evaluation permit and the live-thread ceiling, the thread with its own sized stack, the
/// arena's allocation limit, the interpreter's time box and depth bound, the unwind guard,
/// and the ceilings on the template, the document and the mapped result. There is one
/// evaluation substrate in this service and this is it.
///
/// The failure is reported as TEXT rather than as a modelled error, because a transformer
/// job carries the reason in its own `failed` message: the request that started the job was
/// served, and it is the RUN that could not be performed.
pub(crate) async fn evaluate_mapping_template(
    template: String,
    document: String,
) -> Result<String, String> {
    // The mapped DOCUMENT's text, which is what a job writes to its output file: the
    // JSON-string encoding the response member carries ([`mapped_file_member`]) belongs to
    // the answer `TestMapping` gives its caller and not to the mapped file.
    map_document(TestMappingInput {
        mapping_template: template,
        input_file_content: document,
        // The document a transformer job evaluates a mapping over is the JSON form of the
        // parsed interchange, which is what a JSONata expression navigates.
        file_format: FILE_FORMAT_JSON.to_string(),
    })
    .await
    .map_err(|failure| failure.to_string())
}

/// The `FileFormat` value that declares `inputFileContent` holds an XML document
/// (`com.amazonaws.b2bi#FileFormat` is `XML | JSON | NOT_USED`).
const FILE_FORMAT_XML: &str = "XML";

/// The `FileFormat` value that declares it holds a JSON document — the form a JSONata
/// expression navigates.
const FILE_FORMAT_JSON: &str = "JSON";

/// Wall-clock ceiling on ONE JSONata evaluation, in milliseconds, passed to the
/// interpreter's own time box.
///
/// A mapping template is caller-supplied code and JSONata can express non-terminating
/// recursion, so an evaluation needs a bound that does not depend on the template
/// behaving. The value is this emulator's own choice — AWS documents no evaluation
/// deadline for `TestMapping` — and it is deliberately short: the interpreter checks it
/// between steps, and every step a template can reach here is bounded (see this module's
/// docs), so it is the bound an evaluation cannot outrun.
const EVALUATION_TIME_LIMIT_MS: usize = 500;

/// The runtime-level deadline on one evaluation: the backstop for a single interpreter
/// step that runs long without reaching the interpreter's own time box, which is checked
/// between evaluation steps. Three times [`EVALUATION_TIME_LIMIT_MS`], so a healthy
/// evaluation is always ended by the interpreter's box and this one only fires when that
/// box could not.
const EVALUATION_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_millis(1_500);

/// Ceiling on how deeply one evaluation may nest, passed to the interpreter's own depth
/// box: the second bound on caller-supplied code, and the one that stops unbounded
/// recursion long before it exhausts the evaluation thread's stack. This emulator's own
/// choice, as above.
const MAX_EVALUATION_DEPTH: usize = 256;

/// How many mapping evaluations this emulator runs at once.
///
/// Caller-supplied code is CPU-bound and allocates into a per-evaluation arena, so this
/// permit count is what bounds BOTH the CPU and the resident arenas a flood of requests
/// can occupy: never more than this many evaluations are in flight, whatever the request
/// rate. A request that cannot get a permit within [`PERMIT_WAIT`] is throttled rather
/// than queued for good.
///
/// Four, not one or two: each evaluation's memory is bounded by
/// [`MAX_EVALUATION_ARENA_BYTES`] plus a small multiple for the intermediates the
/// interpreter builds outside the arena, so a handful of them is a bounded footprint —
/// and a test suite or a client that maps a few documents in parallel should not meet
/// the throttle.
const MAX_CONCURRENT_EVALUATIONS: usize = 4;

/// How long a request waits for one of [`MAX_CONCURRENT_EVALUATIONS`] permits.
const PERMIT_WAIT: ::std::time::Duration = ::std::time::Duration::from_millis(2_000);

/// `retryAfterSeconds` on the throttle: the longest evaluation ahead of the caller is
/// [`EVALUATION_DEADLINE`], so a second is enough for a permit to come free.
const THROTTLE_RETRY_AFTER_SECONDS: i32 = 1;

/// The evaluation permits [`MAX_CONCURRENT_EVALUATIONS`] hands out.
///
/// A `static`, not per-operation state: the bound is on this emulator PROCESS. The permit
/// belongs to the REQUEST and is released when the request is answered — including when
/// it is answered by [`EVALUATION_DEADLINE`] rather than by the evaluation — so an
/// evaluation that outruns its caller cannot retire a slot for the life of the process.
/// What bounds the threads such an evaluation leaves behind is [`MAX_LIVE_EVALUATIONS`].
static EVALUATION_PERMITS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_EVALUATIONS);

/// How many evaluation THREADS may exist at once, including any that outran the request
/// that started them.
///
/// The permit recycles with the request, so this is the bound that keeps a flood \u2014 or a
/// template whose last step runs long \u2014 from piling threads and arenas up without limit:
/// a request that would start the (n+1)th is throttled instead. Slack over
/// [`MAX_CONCURRENT_EVALUATIONS`], not a multiple of it, so the resident footprint stays a
/// small multiple of [`MAX_EVALUATION_ARENA_BYTES`] while a slow evaluation handing its
/// slot on does not have to be waited for.
const MAX_LIVE_EVALUATIONS: usize = MAX_CONCURRENT_EVALUATIONS + 2;

/// How many evaluation threads are alive right now, in this process.
static LIVE_EVALUATIONS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One live evaluation thread, counted in [`LIVE_EVALUATIONS`] for as long as the thread
/// runs.
///
/// Constructed BEFORE the thread is spawned and moved into it, so the count falls when
/// the evaluation really ends \u2014 including when it ends long after the request that
/// started it was answered by [`EVALUATION_DEADLINE`]. That is what makes the recycled
/// permit safe: the permit says how many requests may be evaluating, and this says how
/// many threads may exist, so neither a flood nor a wedged evaluation can grow the
/// process without limit, and neither can disable the operation for good.
#[derive(Debug)]
struct LiveEvaluation;

impl LiveEvaluation {
    /// Count one more live evaluation, or `None` when [`MAX_LIVE_EVALUATIONS`] already
    /// are.
    fn start() -> Option<Self> {
        use ::std::sync::atomic::Ordering;
        let mut live = LIVE_EVALUATIONS.load(Ordering::Acquire);
        loop {
            if live >= MAX_LIVE_EVALUATIONS {
                return None;
            }
            match LIVE_EVALUATIONS.compare_exchange_weak(
                live,
                live + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(seen) => live = seen,
            }
        }
    }
}

impl Drop for LiveEvaluation {
    fn drop(&mut self) {
        LIVE_EVALUATIONS.fetch_sub(1, ::std::sync::atomic::Ordering::AcqRel);
    }
}

/// Ceiling on the `mappingTemplate` this emulator will parse, in bytes.
///
/// The model allows `len 0..350000`; this is this emulator's own, smaller bound, still
/// well above any mapping a real transformer carries, and it is what bounds the work the
/// tokenizer and the parse do over one request. How DEEP that parse may recurse is a
/// separate bound ([`MAX_TEMPLATE_NESTING`]), because recursion depth comes from the
/// template's structure rather than from its length.
const MAX_TEMPLATE_BYTES: usize = 16 * 1024;

/// Ceiling on how deeply a `mappingTemplate` may NEST, counted before it is parsed.
///
/// An EARLY, CHEAP refusal, not a safety bound. The JSONata parse is recursive descent,
/// so every nested construct costs a parser frame; what keeps that recursion inside the
/// evaluation thread's stack is [`EVALUATION_STACK_BYTES`], which is sized for one frame
/// per BYTE of the longest template the length ceiling admits and therefore does not
/// depend on this count being right. This refuses the ordinary deep-nesting case before
/// the parse touches hundreds of megabytes of stack, and names a ceiling the caller can
/// act on.
///
/// Set to [`MAX_EVALUATION_DEPTH`]: a template nested deeper than the interpreter will
/// evaluate could not be mapped anyway (its depth box ends the evaluation), so refusing
/// it one step earlier costs no reachable behaviour.
const MAX_TEMPLATE_NESTING: usize = MAX_EVALUATION_DEPTH;

/// Ceiling on the input document this emulator will map, in bytes.
///
/// The interpreter holds the whole parsed document in its arena, several times the
/// document's own size, so this bounds the input-proportional part of an evaluation's
/// memory. The model allows `len 0..5000000`; this is this emulator's own, smaller bound.
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// The stack the evaluation thread is given.
///
/// Sized so that NO template the length ceiling admits can overflow it, with nothing
/// about the template's structure needing to be understood first — because a parser stack
/// overflow ABORTS the process instead of unwinding into a refusal, this bound may not
/// depend on a scan of caller-supplied text being accurate.
///
/// The factor is MEASURED in the test (debug) profile, where frames are largest: a path
/// run of ~2 500 steps and a nesting of 2 000 levels both overflow a 32 MiB stack, so ONE
/// parser frame costs this parser on the order of 13-16 KiB. The most frames a template
/// can ask for is one per BYTE (`(((…`, the cheapest frame there is), so the worst
/// template inside [`MAX_TEMPLATE_BYTES`] needs on the order of 16 384 x 16 KiB = 256 MiB;
/// this is a 2x margin over that. [`MAX_TEMPLATE_NESTING`] refuses ordinary deep nesting
/// earlier and more cheaply, but it is NOT what makes this safe: a template whose
/// structure that scan cannot see still parses inside this stack.
///
/// `tests/all/test_mapping.rs` parses a template at exactly [`MAX_TEMPLATE_BYTES`] in
/// every dense shape — brackets of each kind, prefix operators, nested calls, path,
/// arithmetic, conditional, boolean, concatenation and chain runs, and the shapes that
/// hide their structure from the nesting scan (a quote inside a regex literal or a
/// backtick-quoted name, an unterminated quote) followed by 16 KiB of brackets — and
/// asserts each is ANSWERED with the emulator still serving, so a regression in this
/// margin (an engine upgrade with fatter frames, a new recursion shape) fails the build's
/// tests rather than the serving process.
///
/// The evaluation that follows the parse recurses at most [`MAX_EVALUATION_DEPTH`] frames
/// and needs a small fraction of the same stack. It is RESERVED address space, committed
/// only as the parse touches it — an ordinary template commits a few pages of it, and only
/// a template built to nest thousands of levels deep commits much of it — and at most
/// [`MAX_LIVE_EVALUATIONS`] of these threads exist at once (the live-thread bound, not the
/// permit count: a thread outlives the request that gave up on it), so the whole
/// reservation is [`MAX_LIVE_EVALUATIONS`] times this. A host that will not give the
/// thread its stack is answered with the modelled 500 rather than by serving a wrong
/// mapping.
const EVALUATION_STACK_BYTES: usize = 512 * 1024 * 1024;

/// Ceiling on how much one evaluation may allocate into its arena.
///
/// Installed on the arena as `set_allocation_limit`, so `bumpalo` compares every chunk
/// it is about to request against it and REFUSES before asking the system for memory:
/// the clamp precedes the allocation. A refusal unwinds rather than aborting — `bumpalo`
/// raises it through `Bump::oom` (`panic!("out of memory")`) and through its own
/// `alloc::handle_alloc_error` (`panic!("encountered allocation error: …")`), neither of
/// which is `std::alloc::handle_alloc_error` — and the evaluation runs inside
/// [`::std::panic::catch_unwind`], so the refusal frees the arena and becomes the
/// modelled 400 in [`evaluation_panic`].
///
/// 64 MiB: a document within [`MAX_DOCUMENT_BYTES`] parses into a few times its own
/// size, so this leaves a legitimate mapping ample room while bounding the resident
/// memory of all [`MAX_CONCURRENT_EVALUATIONS`] evaluations to a fixed multiple of it.
const MAX_EVALUATION_ARENA_BYTES: usize = 64 * 1024 * 1024;

/// Ceiling on the mapped document this operation will answer with: the same
/// `len 0..5000000` the model puts on `TestMappingInputFileContent`, since a mapping
/// result is the document a transformer would go on to write.
const MAX_MAPPED_FILE_BYTES: usize = 5_000_000;

/// The three constructs this emulator will not evaluate, as the node each appears under
/// in the engine's parsed expression, with the name a refusal gives it.
///
/// Each is one interpreter step with no bound this handler can place ahead of it — see
/// [`unbounded_construct`] for the reason per entry, and for the MEASURED evidence that
/// every other construct's growth is clamped by the arena's allocation limit before the
/// memory is asked for.
const UNBOUNDED_CONSTRUCT_NODES: &[(&str, &str)] = &[
    // TIME. A regular-expression literal is the only way a template can obtain a regex
    // (the engine's function table has no regex constructor and a JSON document carries
    // no regex value), and the matcher behind it (`regress`) backtracks with no step
    // budget: ONE match of a caller-written pattern can run for an unbounded time inside
    // a single step, which neither the interpreter's between-step budget nor
    // `EVALUATION_DEADLINE` can end — a `std::thread` cannot be cancelled, so such a step
    // would hold one of `MAX_LIVE_EVALUATIONS` threads and a core for the life of the
    // process. `$match`, and `$split`/`$contains`/`$replace` given a PATTERN, all need a
    // literal, so this covers them while the same builtins over plain strings are
    // evaluated.
    ("Regex(", "a regular-expression literal"),
    // TIME. `fn_distinct` keeps a member only if it is not already kept, comparing each
    // member against the ones before it: the one step is QUADRATIC in the member count,
    // and an array inside the arena ceiling holds millions of members, so its wall-clock
    // time is not bounded by the bytes the arena charged — the same uninterruptible-thread
    // consequence as above.
    (r#"Var("distinct")"#, "$distinct"),
    // MEMORY. `fn_pad` builds its result with
    // `chars().cycle().take(width).collect::<String>()` — a `String` on the GLOBAL heap,
    // sized by a NUMBER written in the template rather than by any value it was given, so
    // neither the arena's allocation limit nor the document ceiling precedes it:
    // `$pad("x", 1e12)` asks the system allocator for a terabyte from a 14-byte template,
    // and `std::alloc::handle_alloc_error` ABORTS where the arena's refusal unwinds. It is
    // the one construct whose growth is not arena-charged (measured, see
    // [`unbounded_construct`]), so it is refused rather than evaluated.
    (r#"Var("pad")"#, "$pad"),
];

/// The construct in a PARSED template whose ONE interpreter step this emulator cannot
/// bound, named for the refusal, or `None` when the template uses none.
///
/// Checked after the template is parsed and before it is evaluated, because the
/// interpreter's time box is checked BETWEEN steps and its memory clamp is on the arena:
/// a step that cannot be bounded cannot be interrupted either. The list is
/// [`UNBOUNDED_CONSTRUCT_NODES`], which carries the reason per entry — two whose single
/// step has no time bound (a regular-expression literal, `$distinct`) and one whose
/// buffer is not arena-charged (`$pad`).
///
/// EVERYTHING ELSE IS EVALUATED, including the string-SERIALIZING builtins `$string`, the
/// `&` operator, `$join` and `$replace`, because their growth is charged to the ARENA and
/// `bumpalo` refuses an over-ceiling layout BEFORE asking the system for it. That is
/// MEASURED, not assumed: `tests/all/test_mapping.rs` drives each of them over a 100 KB
/// member aliased a thousand times and the refusal that comes back names
/// [`MAX_EVALUATION_ARENA_BYTES`] (the engine asks the arena for the whole
/// 100 000 000-byte buffer, `bumpalo`'s allocation limit declines it, and the unwind
/// becomes the modelled 400), while the same shape at a hundred aliases is refused by the
/// answer ceiling ([`fits_mapped_ceiling`]). So an aliasing template cannot make one step
/// ask the global allocator for more than the arena ceiling, and the process cannot be
/// ended by one request.
///
/// The claim each evaluated binding has to meet is about TIME as well as space: its one
/// step must run in time at most linear — or `n log n` — in the arena bytes its operands
/// occupy, because the arena ceiling is then what bounds how long that step can run.
/// Reading the engine's own bindings: the ones that take a FUNCTION (`$map`, `$filter`,
/// `$reduce`, `$each`, `$single`, `$sort` with a comparator) call it once per member and
/// every call is an interpreter step, so the time box interrupts them between members; the
/// ones that do their work natively walk their operand ONCE (`$reverse`, `$append`,
/// `$zip`, `$merge`, `$keys`, `$lookup`, `$count`, `$sum`, `$max`, `$min`, the string
/// builtins, the `in` operator and a value comparison), except `$sort` with the engine's
/// own comparator, which is `n log n`. `$distinct` is the one quadratic pass, and it is
/// refused above. The range operator, the one construct whose size comes from a number
/// rather than from data, is capped by the engine itself at 1e7 elements (`jsonata-rs`
/// answers `D2014` past that) inside the same arena ceiling.
///
/// A refused construct is answered with the 501 from [`construct_not_implemented`], the
/// same shape as the missing XSLT engine: the template is valid JSONata and this
/// emulator, not the service, is what will not run it. The modelled 400 stays for a
/// request that is really malformed.
///
/// The decision is read off the ENGINE's own parse ([`::jsonata_rs::JsonAta::ast`],
/// rendered through its `Debug`), never off the template's text: whether a `/` is a regex
/// literal or the division operator, and whether `$pad` is the builtin or part of a
/// longer name, are questions only the engine's lexer can answer, and this asks it rather
/// than guessing. `Debug` escapes the quotes inside a string literal, so a template
/// carrying the text `Var("pad")` in a literal is evaluated normally; a literal
/// containing the quote-free `Regex(`, and a template whose own variable is NAMED after a
/// refused builtin (`($pad := 1; $pad)`), are over-refused, which is the safe direction
/// and is pinned by the tests. A `Debug` rendering is not a stability contract and it is
/// the only view of the parse this crate HAS (`jsonata-rs` declares `mod parser`
/// privately), which is why the interpreter is pinned to the EXACT version these node
/// shapes were read from and why the tests assert both the refusals and the constructs
/// that must NOT be refused.
fn unbounded_construct(parsed: &impl ::std::fmt::Debug) -> Option<&'static str> {
    let rendered = format!("{parsed:?}");
    UNBOUNDED_CONSTRUCT_NODES
        .iter()
        .find(|(node, _)| rendered.contains(node))
        .map(|(_, name)| *name)
}

/// The transformation language a `mappingTemplate` is written in.
///
/// `Mapping$templateLanguage` in this service's model names the two a template can be
/// written in — `JSONATA` and `XSLT` — but `TestMapping` predates `Mapping` and carries
/// no language member, so the language is read off the template itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateLanguage {
    /// A JSONata expression.
    Jsonata,
    /// An XSLT stylesheet.
    Xslt,
}

/// Which language `template` is written in.
///
/// An XSLT stylesheet is an XML document, so it opens with `<` (`<?xml …` or
/// `<xsl:stylesheet …`); `<` in JSONata is only an infix comparison operator, so no
/// JSONata expression can begin with one.
fn template_language(template: &str) -> TemplateLanguage {
    if template.trim_start().starts_with('<') {
        TemplateLanguage::Xslt
    } else {
        TemplateLanguage::Jsonata
    }
}

/// The modelled ceiling on an error message: `ValidationException$Message` targets
/// `ErrorMessage`, which the model constrains to `len 10..1024`.
const MAX_ERROR_MESSAGE_CHARS: usize = 1024;

/// The modelled 400 refusal, carrying `message`.
///
/// This service models its own `ValidationException` and `TestMapping` lists it, so a
/// bad request is refused with the modelled shape rather than the scaffold's synthetic
/// one. The text is clipped to [`MAX_ERROR_MESSAGE_CHARS`] because part of it is the
/// interpreter's own account of the failure, whose length the caller controls.
fn invalid(message: impl Into<String>) -> TestMappingError {
    let message = message.into();
    let message = if message.chars().count() > MAX_ERROR_MESSAGE_CHARS {
        message
            .chars()
            .take(MAX_ERROR_MESSAGE_CHARS - 1)
            .chain(['…'])
            .collect()
    } else {
        message
    };
    TestMappingErrorModeled::ValidationException(ValidationException { message }).into()
}

/// The clause every refusal that carries the JSONata engine's own diagnosis begins with.
///
/// Fixed text: it says which engine refused the mapping and what the service was doing,
/// and it names NO input member — the diagnosis that follows it is the engine's account of
/// the expression, not this service's account of the member the expression arrived in.
const JSONATA_FAILURE: &str = "JSONata error. Failed to map input file:";

/// How the embedded engine marks the end-of-input symbol in its own sentence.
///
/// The engine states an expression that ran out of input as that symbol standing where an
/// operand belongs; the JSONata error catalogue states the same failure as the TOKEN the
/// expression still needed before it ended ([`awaited_closer`]), which is the sentence a
/// caller reads — and the engine itself states that sentence wherever its own parse knows
/// which token was awaited.
const END_OF_INPUT_SYMBOL: &str = "`(end)`";

/// The modelled 400 for a JSONata expression the engine refused: the fixed clause above,
/// the engine's own diagnosis of the expression, and the span of the expression the
/// diagnosis is about ([`jsonata_message`]).
fn jsonata_refusal(template: &str, failure: &::jsonata_rs::Error) -> TestMappingError {
    invalid(jsonata_message(template, &failure.to_string()))
}

/// The whole sentence a refused JSONata expression is reported with.
///
/// `rendered` is the engine's own rendering of the failure, which carries an error code, the
/// position in the expression it stopped at, and the sentence stating what was wrong
/// (`<code> @ <position>: <sentence>`). The code is the engine's internal handle on the
/// sentence and is not reported; the sentence is, with the tokens it quotes written the way
/// the error catalogue writes them, and the position becomes the SPAN of the expression the
/// diagnosis is about: from where its first token begins to one past where the parse
/// stopped.
///
/// A diagnosis carrying no position — which the engine renders for a failure that is about
/// the expression as a whole rather than a place in it — is reported without a span.
fn jsonata_message(template: &str, rendered: &str) -> String {
    let Some((position, sentence)) = engine_diagnosis(rendered) else {
        return format!("{JSONATA_FAILURE} {}", as_a_sentence(rendered));
    };
    // An expression that ran out of input is stated as the token it still needed, which is
    // read off the EXPRESSION — the innermost mark the caller left open. Where it left
    // none open, nothing was awaited and the engine's own sentence stands.
    let awaited = sentence
        .contains(END_OF_INPUT_SYMBOL)
        .then(|| awaited_closer(template))
        .flatten();
    let diagnosis = match awaited {
        Some(closer) => as_a_sentence(&format!(
            "Expected \"{closer}\" before end of expression"
        )),
        None => as_a_sentence(sentence),
    };
    let (from_line, from_column) = expression_start(template);
    let (line, column) = position_in(template, position);
    format!(
        "{JSONATA_FAILURE} {diagnosis} From line {from_line} column {from_column} to line \
         {line} column {column}."
    )
}

/// The token an expression that ran out of input still needed: the closing mark of the
/// INNERMOST bracket the caller left open, and `None` when it left none open.
///
/// Read off the expression itself, because that is where the awaited token is: a bracket
/// the caller opened is a bracket the parse was still inside when the input ended. String
/// literals, backtick-quoted names and comments carry no structure, so each is skipped to
/// its terminator — the same reading [`max_template_nesting`] does of the same bytes, with
/// the same `closing_quote` — and a template whose literal or comment is UNTERMINATED hides
/// whatever follows from this scan, so nothing is claimed to be awaited there.
///
/// Single-pass over a template already bounded by [`MAX_TEMPLATE_BYTES`], nested no deeper
/// than [`MAX_TEMPLATE_NESTING`] (both checked before the engine is handed the template),
/// so the walk and the marks it holds are bounded by those two ceilings.
fn awaited_closer(template: &str) -> Option<char> {
    let bytes = template.as_bytes();
    let mut open: Vec<u8> = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            quote @ (b'"' | b'\'' | b'`') => index = closing_quote(bytes, index, quote)?,
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let end = bytes[index + 2..]
                    .windows(2)
                    .position(|pair| pair == b"*/")?;
                index += 2 + end + 1;
            }
            mark @ (b'(' | b'[' | b'{') => open.push(mark),
            b')' | b']' | b'}' => {
                open.pop();
            }
            _ => {}
        }
        index += 1;
    }
    match open.last()? {
        b'(' => Some(')'),
        b'[' => Some(']'),
        _ => Some('}'),
    }
}

/// The position the engine stopped at and the sentence it states, read off its own
/// rendering (`<code> @ <position>: <sentence>`), or `None` when it rendered no position.
fn engine_diagnosis(rendered: &str) -> Option<(usize, &str)> {
    let (code, sentence) = rendered.split_once(": ")?;
    let (_, position) = code.split_once(" @ ")?;
    Some((position.trim().parse().ok()?, sentence))
}

/// One diagnosis as a sentence: the tokens the engine quotes in backticks written in the
/// double quotes the error catalogue states them in, and a full stop to close it.
fn as_a_sentence(diagnosis: &str) -> String {
    let stated = diagnosis.trim().replace('`', "\"");
    if stated.ends_with('.') {
        stated
    } else {
        format!("{stated}.")
    }
}

/// Where the expression's first token begins, as a line and a column counted from one.
///
/// The span a diagnosis carries is the span of the EXPRESSION, so it starts at the first
/// token rather than at the first byte: leading whitespace is not part of the expression,
/// and neither is the `$` that introduces a variable or function name — the name is the
/// token.
fn expression_start(template: &str) -> (usize, usize) {
    let first = template
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .map(|(offset, character)| match character {
            '$' => offset + '$'.len_utf8(),
            _ => offset,
        })
        .unwrap_or(0);
    position_in(template, first.min(template.len()))
}

/// The one-based line, and the one-based column in bytes, of byte `offset` of `template`.
///
/// The engine reports the position it stopped at as the offset of the byte AFTER the token
/// it read, so the offset one past the template's last byte — where a parse that ran out of
/// input stops — is a position this answers for. An offset inside a multi-byte character
/// names that character, so no position the engine reports can split one.
fn position_in(template: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(template.len());
    while offset > 0 && !template.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &template[..offset];
    let line = 1 + before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |at| at + '\n'.len_utf8());
    (line, 1 + before.len() - line_start)
}

/// The 501 for a template in a language this emulator has no engine for.
///
/// `TestMapping` itself is implemented, so the wording names the engine that is missing
/// rather than the operation: a caller sending a JSONata template never sees it.
fn language_not_implemented(language: TemplateLanguage) -> TestMappingError {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "TestMapping evaluates JSONATA mapping templates; a {language:?} mappingTemplate \
             needs an engine this emulator does not have, so that template language is not \
             implemented here"
        ),
    })
}

/// The modelled length `MappingTemplate` carries (`len 0..350000`), named so a refusal
/// can say how far below it this emulator's own ceiling sits.
const MODELLED_MAX_TEMPLATE_BYTES: usize = 350_000;

/// The modelled length `TestMappingInputFileContent` carries (`len 0..5000000`), named
/// for the same reason.
const MODELLED_MAX_DOCUMENT_BYTES: usize = 5_000_000;

/// The 501 for a construct this emulator's evaluator will not run.
///
/// The same shape as the missing XSLT engine, and for the same reason: the template is
/// valid JSONata, so telling the caller its request is invalid would be a lie about the
/// request. What is missing is a bound this emulator can place on ONE step of that
/// construct (see [`unbounded_construct`]) — the modelled 400 stays for a template or
/// document that really is malformed.
fn construct_not_implemented(construct: &str) -> TestMappingError {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "TestMapping evaluates JSONATA mapping templates, but this emulator does not \
             evaluate {construct}: it is one interpreter step whose time or memory it cannot \
             bound, so that construct is not implemented here"
        ),
    })
}

/// The 501 for a request this emulator has no engine for, naming what is missing.
///
/// The companion of [`language_not_implemented`] for a request whose DOCUMENT, rather
/// than its template, needs the engine this emulator does not have.
fn capability_not_implemented(detail: String) -> TestMappingError {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "TestMapping maps what this emulator has an engine for: {detail}, so that request is \
             not implemented here"
        ),
    })
}

/// The 501 for a request inside the MODELLED lengths that is past one of this emulator's
/// own, smaller ceilings.
///
/// The service would map it; this emulator declines to attempt it, which is a capability
/// it lacks rather than a defect in the request — so it answers the way the missing XSLT
/// engine does. A failure found while EVALUATING (the time box, the depth box, the arena
/// ceiling, a result past the answer ceiling) is the modelled 400 instead: there the
/// template, over this document, is what did not come back inside the bounds.
fn ceiling_not_implemented(detail: String) -> TestMappingError {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "TestMapping maps what this emulator can hold in one evaluation: {detail}, so that \
             request is not implemented here"
        ),
    })
}

/// `mappedFileContent` for an evaluated result: the mapped document as the TEXT a caller
/// reads.
///
/// The member is modelled as a plain `String` and what it carries is the mapped DOCUMENT,
/// so the answer is that document rendered to text — pretty-printed with two-space
/// indentation and a space after each member's colon, the layout the recorded answer
/// carries. (The generated mapping TEMPLATE, which `CreateStarterMappingTemplate` writes
/// with its own generator, is recorded with a space on BOTH sides of that colon instead,
/// so the two layouts are kept apart in [`crate::state::documents::to_document_text`].)
///
/// This is the document's own text; what the response MEMBER carries is that text
/// JSON-string-encoded — see [`mapped_file_member`], which is the last step before the
/// answer, so the text itself is what a transformer job writes to its output file.
///
/// Every object's members are written in sorted key order. That is NOT the order the real
/// service answers — it writes the members in the order the mapping constructed them — and
/// this emulator cannot reproduce that order: the interpreter holds an object's members in
/// a hash map, so its own order varies between evaluations, and `jsonata-rs` declares the
/// parsed expression's `mod parser` privately, so there is no typed view of the template's
/// construction order to take it from either. Sorting is what keeps the same request
/// answering the same text; the member ORDER is a known divergence rather than a rule this
/// handler implements.
///
/// Two results are passed through unchanged: the empty string the interpreter writes
/// for an expression with no value, and anything the JSON reader will not take back
/// (a result nested past its recursion limit) — answered as the interpreter wrote it
/// rather than refused.
fn mapped_document_text(serialized: String) -> String {
    match ::serde_json::from_str::<::serde_json::Value>(&serialized) {
        Ok(parsed) => {
            ::serde_json::to_string_pretty(&with_sorted_members(parsed)).unwrap_or(serialized)
        }
        Err(_) => serialized,
    }
}

/// The required `mappedFileContent` member for a mapped document whose text is `document`:
/// that text as a JSON STRING LITERAL.
///
/// The mapped file travels as text inside a `String` member, and the recorded answers
/// carry that text JSON-string-encoded — a caller reads the member, parses it once to
/// obtain the mapped file's text and parses that text to obtain the document — so the
/// encoding is applied here, on the member, and nowhere else: a mapped document that is
/// itself a JSON scalar (`"PO-10042"`, `42`) is then still a value the caller recovers by
/// the same two steps as a mapped object, rather than one that is or is not readable
/// depending on the result's JSON type.
///
/// Escaping a string cannot fail, so the encoding is written through `Value`'s own
/// `Display`, which needs no fallback.
fn mapped_file_member(document: String) -> String {
    ::serde_json::Value::String(document).to_string()
}

/// `value` with every object's members in sorted key order.
///
/// The sort is explicit because `serde_json::Map` keeps insertion order whenever anything
/// in the build turns on the crate's `preserve_order` feature.
fn with_sorted_members(value: ::serde_json::Value) -> ::serde_json::Value {
    match value {
        ::serde_json::Value::Array(items) => {
            ::serde_json::Value::Array(items.into_iter().map(with_sorted_members).collect())
        }
        ::serde_json::Value::Object(members) => {
            let mut pairs: Vec<(String, ::serde_json::Value)> = members.into_iter().collect();
            pairs.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            ::serde_json::Value::Object(
                pairs
                    .into_iter()
                    .map(|(key, value)| (key, with_sorted_members(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

/// The modelled 429 refusal: this emulator is already running
/// [`MAX_CONCURRENT_EVALUATIONS`] mappings and no permit came free in time.
///
/// `ThrottlingException` is what `TestMapping` models for "the service would not take
/// this request now", and it is retryable, so a client backs off and resends.
fn throttled() -> TestMappingError {
    TestMappingErrorModeled::ThrottlingException(ThrottlingException {
        message: format!(
            "this emulator evaluates at most {MAX_CONCURRENT_EVALUATIONS} TestMapping mappings at \
             once and no evaluation slot came free within {} ms",
            PERMIT_WAIT.as_millis()
        ),
        retry_after_seconds: Some(THROTTLE_RETRY_AFTER_SECONDS),
    })
    .into()
}

/// The modelled 500: the evaluation could not be run or did not come back, which says
/// nothing about the caller's template, so it is reported as the server fault it is.
fn server_fault(message: &str) -> TestMappingError {
    TestMappingErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
        retry_after_seconds: None,
    })
    .into()
}

/// Whether the ANSWERED document for `value` fits in `budget` bytes, walking the result
/// and stopping the moment it would not.
///
/// The ceiling on the answered document has to be decided BEFORE the document exists:
/// [`::jsonata_rs::Value::serialize`] builds it as a `String` on the global heap, which
/// the arena's allocation limit does not govern, and the interpreter holds values behind
/// arena references — so a template that aliases one value into many places has a small
/// arena footprint and a large serialization. This charges `budget` the way the answer is
/// written, so an oversized result is refused before any of those bytes are allocated.
///
/// `indent` is the level this value sits at, because the answer is the PRETTY rendering
/// ([`mapped_document_text`]): each member of a container costs a newline and two spaces
/// per level on top of what the compact form costs, which is the part of the answer a
/// deeply nested result grows fastest in and therefore the part that may not go
/// uncharged.
///
/// The walk is bounded by the ceiling it enforces: every node charges at least one byte,
/// so it visits at most `budget` nodes, and it refuses past `depth` levels of nesting
/// rather than recursing further.
fn fits_mapped_ceiling<'a>(
    value: &'a ::jsonata_rs::Value<'a>,
    budget: &mut usize,
    depth: usize,
    indent: usize,
) -> bool {
    /// Charge `bytes` against the remaining budget.
    fn charge(budget: &mut usize, bytes: usize) -> bool {
        match budget.checked_sub(bytes) {
            Some(left) => {
                *budget = left;
                true
            }
            None => false,
        }
    }

    /// What the serializer writes for `text`, quotes included: a byte it escapes with a
    /// `\` costs two, a control byte it writes as `\u00xx` costs six.
    fn quoted_len(text: &str) -> usize {
        2 + text
            .bytes()
            .map(|byte| match byte {
                b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 2,
                0x00..=0x1f => 6,
                _ => 1,
            })
            .sum::<usize>()
    }

    /// What the serializer writes for a number: its digits, not the widest number's.
    ///
    /// The serializer writes a finite number with at most 15 significant digits, and
    /// nothing for a non-finite one. Charging each number its own width rather than the
    /// widest possible keeps a result of many small numbers inside the ceiling it
    /// actually serializes to.
    fn number_len(number: f64) -> usize {
        if !number.is_finite() {
            // `null`, which is what the serializer writes for a non-finite number.
            return 4;
        }
        let magnitude = number.abs();
        let sign = usize::from(number.is_sign_negative());
        if number.fract() == 0.0 && magnitude < 1e15 {
            // The integer digits, which is the whole rendering.
            let digits = if magnitude < 1.0 {
                1
            } else {
                magnitude.log10().floor() as usize + 1
            };
            return sign + digits;
        }
        // A fraction or an exponent: the widest the 15-significant-digit rendering gets
        // (sign, leading digit, point, 14 digits, `e`, exponent sign, three exponent
        // digits).
        sign + 24
    }

    if depth == 0 {
        return false;
    }
    // `Undefined` is the one value the serializer writes nothing for.
    if value.is_undefined() {
        return true;
    }
    if value.is_null() {
        return charge(budget, 4);
    }
    if value.is_bool() {
        return charge(budget, 5);
    }
    if value.is_number() {
        return charge(budget, number_len(value.as_f64()));
    }
    if value.is_string() {
        return charge(budget, quoted_len(&value.as_str()));
    }
    if value.is_array() {
        // The brackets, then for each element a comma, the newline and the indentation
        // the answer is written with, and the element itself.
        if !charge(budget, 2) {
            return false;
        }
        let mut members = 0usize;
        for member in value.members() {
            members += 1;
            if !charge(budget, 1 + 1 + 2 * (indent + 1))
                || !fits_mapped_ceiling(member, budget, depth - 1, indent + 1)
            {
                return false;
            }
        }
        // The newline and indentation before the closing bracket, which a container with
        // no members is written without.
        return members == 0 || charge(budget, 1 + 2 * indent);
    }
    if value.is_object() {
        if !charge(budget, 2) {
            return false;
        }
        let mut members = 0usize;
        for (key, member) in value.entries() {
            members += 1;
            // The comma, the newline and indentation, the quoted key and the `: ` after
            // it, then the member.
            if !charge(budget, 1 + 1 + 2 * (indent + 1) + quoted_len(key) + 2)
                || !fits_mapped_ceiling(member, budget, depth - 1, indent + 1)
            {
                return false;
            }
        }
        return members == 0 || charge(budget, 1 + 2 * indent);
    }
    // A function value, which the serializer writes as `""`, or a regex value, which it
    // writes as its pattern. Charged a fixed cost rather than measured, because the typed
    // accessors panic off their own kind — and the post-serialization backstop in
    // `evaluate_jsonata` is what catches any kind this accounting under-charges.
    charge(budget, 2)
}

/// Evaluate `template` over `document` and serialize the result.
///
/// `document` is the JSON text the caller sent (already checked to be well formed), or
/// `None` when `inputFileContent` is empty — the model allows a zero-length one, and an
/// expression that reads nothing from the input still has a value.
///
/// Every failure here is in the caller's template or document, so each is refused with
/// the modelled `ValidationException` carrying the interpreter's own account of it.
fn evaluate_jsonata(template: &str, document: Option<&str>) -> Result<String, TestMappingError> {
    // The document's well-formedness, checked here rather than on the async worker: it
    // is also the bound on how deeply the document may nest, since the reader refuses
    // past its own recursion limit, so neither this parse nor the interpreter's can be
    // driven arbitrarily deep.
    if let Some(document) = document {
        // The JSON parser's own diagnostic IS the refusal, the same reading the conversion
        // source path reports (`crate::state::documents::json_parse_diagnostic`): it says
        // what the parse stopped on and where, with the submitted document withheld.
        crate::state::documents::parse_json(document).map_err(invalid)?;
    }

    let arena = ::bumpalo::Bump::new();
    // The memory clamp, installed before a single byte is allocated into the arena: see
    // `MAX_EVALUATION_ARENA_BYTES` for how `bumpalo` refuses past it, and why that
    // refusal unwinds into `evaluation_panic` rather than ending the process.
    arena.set_allocation_limit(Some(MAX_EVALUATION_ARENA_BYTES));

    // The parse has its OWN unwind guard, because a template the interpreter's parser
    // cannot handle is a template the caller cannot use: `jsonata-rs` answers some
    // malformed expressions with an error and reaches `unreachable!()` on others (its
    // `parser::symbol`), and both mean the same thing to the caller, so both are the
    // modelled 400 naming the member rather than a server fault.
    let parsed = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
        ::jsonata_rs::JsonAta::new(template, &arena)
    }))
    .map_err(|_| {
        invalid(
            "mappingTemplate is not a valid JSONata expression: the interpreter could not parse it",
        )
    })?;
    let expression = parsed.map_err(|failure| jsonata_refusal(template, &failure))?;

    // Read off the parse, before anything is evaluated: a step this emulator cannot bound
    // cannot be interrupted either. The answer is the 501 naming the construct, not a
    // 400: the template is valid JSONata.
    if let Some(construct) = unbounded_construct(expression.ast()) {
        return Err(construct_not_implemented(construct));
    }

    let mapped = expression
        .evaluate_timeboxed(
            document,
            Some(MAX_EVALUATION_DEPTH),
            Some(EVALUATION_TIME_LIMIT_MS),
        )
        .map_err(|failure| match failure {
            ::jsonata_rs::Error::U1001Timeout => invalid(format!(
                "mappingTemplate did not finish evaluating within the \
                 {EVALUATION_TIME_LIMIT_MS} ms this emulator allows one TestMapping evaluation"
            )),
            ::jsonata_rs::Error::U1001StackOverflow => invalid(format!(
                "mappingTemplate nested deeper than the {MAX_EVALUATION_DEPTH} evaluations \
                 this emulator allows one TestMapping evaluation"
            )),
            other => jsonata_refusal(template, &other),
        })?;

    // The result's size is decided BEFORE it is serialized: `Value::serialize` builds the
    // whole document as a `String` on the global heap, which the arena's allocation limit
    // does not see, so the ceiling has to be enforced on the value rather than on the
    // string. The walk refuses for either of two reasons — the byte budget, or nesting
    // past the depth it is given — so the refusal names both ceilings rather than
    // attributing it to the one the caller may not have met.
    let mut budget = MAX_MAPPED_FILE_BYTES;
    if !fits_mapped_ceiling(mapped, &mut budget, MAX_EVALUATION_DEPTH, 0) {
        return Err(invalid(format!(
            "the mapping would produce more than the {MAX_MAPPED_FILE_BYTES} bytes this emulator \
             will answer with, or a document nested deeper than its {MAX_EVALUATION_DEPTH} levels"
        )));
    }
    let serialized = mapped.serialize(false);

    // The backstop for the walk above: it charges at least what the answer is written
    // with, so this cannot fire, and it is kept so an accounting slip cannot answer past
    // the ceiling either. Checked on the interpreter's own serialization first, which is
    // never longer than the answer, so nothing oversized reaches the rendering below.
    if serialized.len() > MAX_MAPPED_FILE_BYTES {
        return Err(invalid(format!(
            "the mapping produced {} bytes, past the {MAX_MAPPED_FILE_BYTES} bytes this emulator \
             will answer with",
            serialized.len()
        )));
    }
    let answered = mapped_document_text(serialized);
    if answered.len() > MAX_MAPPED_FILE_BYTES {
        return Err(invalid(format!(
            "the mapping produced {} bytes, past the {MAX_MAPPED_FILE_BYTES} bytes this emulator \
             will answer with",
            answered.len()
        )));
    }
    Ok(answered)
}

/// The whole containment above needs unwinding panics: `catch_unwind` cannot intercept
/// an aborting panic, so a profile built with `panic = "abort"` would turn the arena's
/// refusal — and the interpreter's own `unreachable!()` on some malformed templates —
/// into the death of the serving process instead of the modelled error. The requirement
/// is asserted here rather than left implicit.
#[cfg(panic = "abort")]
compile_error!(
    "TestMapping bounds caller-supplied code with catch_unwind, so this crate requires \
     panic = \"unwind\" (the default)"
);

/// Evaluate on this thread, with no panic able to leave it.
///
/// The one panic a request can provoke is the arena refusing an allocation past
/// [`MAX_EVALUATION_ARENA_BYTES`], which unwinds (freeing the arena) and is answered by
/// [`evaluation_panic`] as the modelled 400. Catching it here, rather than letting the
/// thread die, is also what keeps a single request from ending the serving process.
fn evaluate_bounded(template: &str, document: Option<&str>) -> Result<String, TestMappingError> {
    ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
        evaluate_jsonata(template, document)
    }))
    .unwrap_or_else(|payload| Err(evaluation_panic(payload)))
}

/// The modelled error for an evaluation that unwound.
///
/// `bumpalo` raises an allocation it will not make as `panic!("out of memory")` or
/// `panic!("encountered allocation error: …")`, so a template that asked for more memory
/// than this emulator allows is refused with the modelled 400 naming that ceiling. Any
/// other unwind is not about the request, and is the modelled 500.
fn evaluation_panic(payload: Box<dyn ::std::any::Any + Send>) -> TestMappingError {
    let reported = payload
        .downcast_ref::<&'static str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    if reported.contains("out of memory") || reported.contains("allocation error") {
        ::tracing::warn!(
            operation = OPERATION,
            ceiling_bytes = MAX_EVALUATION_ARENA_BYTES,
            "a mapping evaluation asked for more memory than the arena ceiling allows"
        );
        return invalid(format!(
            "evaluating mappingTemplate over inputFileContent needed more than the \
             {MAX_EVALUATION_ARENA_BYTES} bytes this emulator allows one TestMapping evaluation"
        ));
    }
    ::tracing::error!(
        operation = OPERATION,
        panic = ?reported,
        "a mapping evaluation panicked"
    );
    server_fault("the mapping evaluation did not complete")
}

/// How deeply `template` nests, counted without parsing it.
///
/// An EARLY refusal for the ordinary deep-nesting case, deliberately an OVER-count rather
/// than an exact one: every construct that could cost the recursive-descent parser a
/// frame adds to the depth, and nothing that cannot is counted.
///
/// * a bracket of any kind (`(`, `[`, `{`) opens a nested expression and its closer ends
///   it, which is the depth a parser frame holds;
/// * a run of prefix operators (`-`, `!`) nests the same way without a bracket
///   (`---------1`), so a run is charged on top of the bracket depth it sits at;
/// * a string literal (quoted or backtick-quoted, the engine's two spellings for a name
///   or a literal) and a `/* … */` comment are SKIPPED: a bracket inside one is text, not
///   structure, and charging it would refuse a template over its own data. An
///   UNTERMINATED literal or comment makes the count FAIL CLOSED — it answers
///   [`usize::MAX`], which the caller refuses with the modelled 400 naming the member
///   (such a template is not valid JSONata, and the engine refuses it too). The scan is
///   single-pass, so no input makes it rescan, and the bytes after an unterminated
///   literal are bytes this scanner cannot read as structure while the engine's parser
///   can: a template whose structure is hidden that way would otherwise pass this check
///   and commit parser stack for every byte of it.
///
/// This count is NOT the only thing keeping the parse inside the evaluation thread's
/// stack: it reads caller-supplied text with a scanner that is not the engine's lexer, so
/// a quote in a token the lexer reads differently (inside a regular-expression literal,
/// say) can still hide structure from it. [`EVALUATION_STACK_BYTES`] is sized for one
/// parser frame per BYTE of the longest template this emulator accepts for exactly that
/// reason, so a template this count under-reads is still parsed and answered rather than
/// aborting the process — `tests/all/test_mapping.rs` pins that with the hiding shapes
/// themselves. It is counted UNDER the evaluation permit, so this walk is covered by the
/// same concurrency bound as the evaluation itself.
fn max_template_nesting(template: &str) -> usize {
    let bytes = template.as_bytes();
    let mut depth = 0usize;
    let mut deepest = 0usize;
    let mut prefix_run = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            // A string literal or a backtick-quoted name: skipped to its closing quote,
            // honouring `\` escapes. An unterminated one refuses the template, so the
            // scan never looks at a byte twice and never reads past structure it cannot
            // see.
            quote @ (b'"' | b'\'' | b'`') => {
                prefix_run = 0;
                match closing_quote(bytes, index, quote) {
                    Some(closed) => index = closed,
                    None => return usize::MAX,
                }
            }
            // A comment: skipped to its `*/`. An unterminated one refuses, as above.
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                prefix_run = 0;
                match bytes[index + 2..].windows(2).position(|pair| pair == b"*/") {
                    Some(end) => index += 2 + end + 1,
                    None => return usize::MAX,
                }
            }
            b'(' | b'[' | b'{' => {
                prefix_run = 0;
                depth += 1;
                deepest = deepest.max(depth);
            }
            b')' | b']' | b'}' => {
                prefix_run = 0;
                depth = depth.saturating_sub(1);
            }
            b'-' | b'!' => {
                prefix_run += 1;
                deepest = deepest.max(depth + prefix_run);
            }
            byte => {
                // Whitespace does not break a prefix run (`- - - 1` is still a chain);
                // anything else does.
                if !byte.is_ascii_whitespace() {
                    prefix_run = 0;
                }
            }
        }
        index += 1;
    }
    deepest
}

/// The index of the quote that closes the literal opened by `quote` at `open`, or `None`
/// when nothing closes it.
///
/// `\` escapes the byte after it, which is how the engine's own lexer reads a literal, so
/// `"a\"b"` is one literal rather than two.
fn closing_quote(bytes: &[u8], open: usize, quote: u8) -> Option<usize> {
    let mut index = open + 1;
    while index < bytes.len() {
        if bytes[index] == quote {
            return Some(index);
        }
        index += usize::from(bytes[index] == b'\\') + 1;
    }
    None
}

/// The `TestMapping` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TestMapping<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TestMapping<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TestMapping<Eng>
{
    type Input = TestMappingInput;
    type Output = TestMappingOutput;
    type Error = TestMappingError;

    const NAME: &'static str = "TestMapping";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_MAPPING_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_MAPPING_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Authorization first: nothing is evaluated for a caller IAM refuses. A mapping
        // dry run addresses no resource ARN, so the request is matched against the policy
        // wildcard — the same decision `TestParsing` takes, which has the same shape.
        if let Err(denied) =
            crate::state::authorize(&*self.engine, ctx, Self::NAME, crate::state::ANY_RESOURCE)
        {
            return Err(TestMappingErrorModeled::AccessDeniedException(denied).into());
        }

        // Beyond that decision a mapping dry run is a pure function of its request: no
        // state of this service is read or written here.

        let template_bytes = input.mapping_template.len();
        let document_bytes = input.input_file_content.len();
        // The mapped document's TEXT, encoded into the member the answer carries it in as
        // the last step before the answer: see [`mapped_file_member`].
        let answered = map_document(input).await.map(mapped_file_member);

        // One access record per request, whatever the outcome: the sizes and the answer,
        // never the caller's template or document, which are the request's own data.
        match &answered {
            Ok(mapped) => ::tracing::info!(
                operation = Self::NAME,
                template_bytes,
                document_bytes,
                mapped_bytes = mapped.len(),
                outcome = "mapped",
                "evaluated a mapping template"
            ),
            Err(refusal) => ::tracing::info!(
                operation = Self::NAME,
                template_bytes,
                document_bytes,
                status = ::smithy_server_runtime::OperationError::http_status(refusal),
                outcome = "refused",
                "refused a mapping request"
            ),
        }
        answered.map(|mapped_file_content| TestMappingOutput {
            mapped_file_content,
        })
    }
}

/// The operation's body — the mapped document's TEXT — so [`TestMapping::handle`] can
/// record one access event for every answer it gives without that record being repeated at
/// each refusal, and so the other operation of this service that evaluates a mapping
/// ([`evaluate_mapping_template`]) shares this one with it.
async fn map_document(mut input: TestMappingInput) -> Result<String, TestMappingError> {
    // The engine is chosen by the template's own language, not by `fileFormat`.
    let language = template_language(&input.mapping_template);
    if language != TemplateLanguage::Jsonata {
        return Err(language_not_implemented(language));
    }

    // JSONata evaluates over a JSON document, so an input the caller declares to be XML
    // needs an XSLT template, which needs an engine this emulator does not have: the
    // service would map that request, so it is answered with the 501 naming the missing
    // capability rather than with a 400 that would call a valid request invalid — the same
    // shape as the XSLT template above. `NOT_USED` declares no format, and is served as
    // the JSON the template needs.
    if input.file_format == FILE_FORMAT_XML {
        return Err(capability_not_implemented(format!(
            "fileFormat {FILE_FORMAT_XML} declares an XML inputFileContent, which only an XSLT \
             mappingTemplate can be evaluated over and this emulator has no XSLT engine"
        )));
    }

    // Both size bounds are checked BEFORE anything parses the request: the template bound
    // is what keeps worst-case parser recursion inside the evaluation thread's stack, and
    // the document bound is what keeps the input-proportional part of the evaluation's
    // memory bounded. A request inside the MODELLED lengths that meets one of them is
    // work this emulator declines to attempt at all, so it is answered with the 501 that
    // names the ceiling — the same shape as the missing XSLT engine — rather than with a
    // 400 that would call a valid request invalid.
    if input.mapping_template.len() > MAX_TEMPLATE_BYTES {
        return Err(ceiling_not_implemented(format!(
            "mappingTemplate is {} bytes and this emulator parses at most {MAX_TEMPLATE_BYTES}, \
             below the {} the model allows",
            input.mapping_template.len(),
            MODELLED_MAX_TEMPLATE_BYTES
        )));
    }
    if input.input_file_content.len() > MAX_DOCUMENT_BYTES {
        return Err(ceiling_not_implemented(format!(
            "inputFileContent is {} bytes and this emulator maps at most {MAX_DOCUMENT_BYTES}, \
             below the {} the model allows",
            input.input_file_content.len(),
            MODELLED_MAX_DOCUMENT_BYTES
        )));
    }

    // One of `MAX_CONCURRENT_EVALUATIONS` permits, or a throttle: caller-supplied code
    // does not get to occupy this process without limit, and NOTHING caller-sized is
    // walked before this point — the nesting count below is under the permit too, so the
    // concurrency bound covers every byte of caller-controlled work this operation does.
    // The permit is held by THIS request — it is dropped when this function returns,
    // whether the evaluation answered or the deadline did — so an evaluation that outruns
    // its caller cannot retire a slot for the life of the process.
    let _permit = match ::tokio::time::timeout(PERMIT_WAIT, EVALUATION_PERMITS.acquire()).await {
        Ok(Ok(permit)) => permit,
        // `Err` is the wait elapsing; `Ok(Err(..))` is a closed semaphore, which a static
        // one never is. Either way this request gets no slot.
        Ok(Err(_)) | Err(_) => return Err(throttled()),
    };

    // How deep the parse may recurse is counted here, BEFORE the template reaches the
    // parser: a template nested deeper than this emulator will evaluate is work it
    // declines to attempt, so it is answered with the 501 naming that ceiling — the same
    // shape as the two size ceilings above, and for the same reason: the template is
    // valid JSONata. The interpreter's OWN depth box, which fires while evaluating,
    // stays the modelled 400. The walk is single-pass over the template's bytes, and it
    // FAILS CLOSED (`usize::MAX`) on a template whose structure it cannot read past — an
    // unterminated literal or comment — rather than reporting the depth it got to.
    let nesting = max_template_nesting(&input.mapping_template);
    if nesting == usize::MAX {
        // The scan cannot read past an unterminated literal or comment, and the bytes
        // after one are bytes the engine's parser WOULD recurse over, so the template is
        // refused here rather than scanned past. It is also not a valid JSONata
        // expression — the engine refuses it too — so this is the modelled 400 naming the
        // member, not the 501 the nesting ceiling gives.
        return Err(invalid(
            "mappingTemplate is not a valid JSONata expression: it carries an unterminated \
             string literal or comment",
        ));
    }
    if nesting > MAX_TEMPLATE_NESTING {
        return Err(ceiling_not_implemented(format!(
            "mappingTemplate nests {nesting} levels deep and this emulator parses and evaluates at \
             most {MAX_TEMPLATE_NESTING} levels"
        )));
    }

    // A zero-length `inputFileContent` is within the modelled `len 0..5000000`, and means
    // "no input document". Anything else, whitespace included, is a document and goes
    // through the well-formedness check on the evaluation thread, under the permit. Moved,
    // not copied: nothing reads the request's strings after this.
    let document = (!input.input_file_content.is_empty())
        .then(|| ::std::mem::take(&mut input.input_file_content));

    // And the bound the recycled permit needs beside it: how many evaluation THREADS
    // exist, counting any that outran the request that started them. A request that would
    // start one too many is throttled rather than allowed to pile threads and arenas up.
    let live = LiveEvaluation::start();
    let Some(live) = live else {
        ::tracing::warn!(
            operation = OPERATION,
            live = MAX_LIVE_EVALUATIONS,
            "every evaluation thread is still running; throttling this mapping"
        );
        return Err(throttled());
    };

    // The evaluation runs on a thread of its own, NOT on the async worker that serves
    // this request: it is caller-supplied code with no await point in it, so on the
    // worker it would hold that worker for its whole budget, and health checks, other
    // requests and shutdown would have nowhere to land. The thread also gets an
    // explicitly sized stack, which the worker and the blocking pool cannot offer (see
    // `EVALUATION_STACK_BYTES`).
    let (sender, receiver) = ::tokio::sync::oneshot::channel();
    let template = input.mapping_template;
    let spawned = ::std::thread::Builder::new()
        .name("b2bi-mapping".to_string())
        .stack_size(EVALUATION_STACK_BYTES)
        .spawn(move || {
            let mapped = evaluate_bounded(&template, document.as_deref());
            // Counted down where the evaluation ENDS, not where the caller stops waiting:
            // a template still running still occupies a thread.
            drop(live);
            // The receiver is gone when the caller's deadline fired first; the result is
            // simply dropped then.
            let _ = sender.send(mapped);
        });
    if let Err(error) = spawned {
        ::tracing::error!(
            operation = OPERATION,
            error = %error,
            "could not start a mapping evaluation"
        );
        return Err(server_fault("the mapping evaluation could not be started"));
    }

    match ::tokio::time::timeout(EVALUATION_DEADLINE, receiver).await {
        Ok(Ok(mapped)) => mapped,
        // The thread ended without sending: the evaluation panicked. That says nothing
        // about the template, so it is a server fault, and the arena it was allocating
        // into is freed by the unwind.
        Ok(Err(_)) => {
            ::tracing::error!(
                operation = OPERATION,
                "a mapping evaluation ended without a result"
            );
            Err(server_fault("the mapping evaluation did not complete"))
        }
        // The runtime-level backstop: the interpreter's own time box did not end the
        // evaluation, so the caller is answered instead of waiting on it. The permit is
        // released with this answer, so the next caller is not throttled on a slot this
        // evaluation no longer needs; the THREAD it is still running on is counted until
        // it ends, and the wait is recorded so a wedged evaluation is observable.
        Err(_) => {
            ::tracing::warn!(
                operation = OPERATION,
                deadline_ms = EVALUATION_DEADLINE.as_millis(),
                live_evaluations = LIVE_EVALUATIONS.load(::std::sync::atomic::Ordering::Relaxed),
                "a mapping evaluation outran the deadline and is still running"
            );
            Err(invalid(format!(
                "mappingTemplate did not finish evaluating within the {} ms this emulator allows \
                 one TestMapping evaluation",
                EVALUATION_DEADLINE.as_millis()
            )))
        }
    }
}
