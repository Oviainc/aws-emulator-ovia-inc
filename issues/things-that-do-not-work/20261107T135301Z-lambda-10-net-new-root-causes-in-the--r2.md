Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.package-non-ascii-file-names-preserved-byte-exact lambda.package-symlinks-extracted-as-symlinks-and-followed lambda.python-handler-in-nested-package-resolves-relative-imports-and-files lambda.python-module-state-is-per-version-not-shared-with-latest lambda.python-runtime-family-every-supported-minor-runs-identically lambda.task-root-and-runtime-dir-read-only-while-tmp-writable-rejected lambda.tmp-not-shared-between-two-distinct-functions lambda.tmp-persists-across-warm lambda.tmp-writable lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected lambda.package-file-modes-normalized-so-every-file-is-readable-and-executable lambda.recursion-config-terminate-stops-a-self-invocation-chain lambda.snapstart-python-version-restores-with-before-snapshot-state-and-a-restore-report lambda.snapstart-restored-version-reuses-the-snapshot-init lambda.two-layers-sharing-a-path-resolve-to-the-last-layer-at-runtime lambda.nodejs-process-exit-reports-exit-error-and-reinitializes-rejected lambda.nodejs-warm-environment-reuses-module-scope-and-tmp lambda.out-of-memory-kill-discards-the-execution-environment lambda.provisioned-concurrency-invocation-reports-no-init-duration lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state lambda.stream-provisioned-environment-reports-provisioned-initialization-type lambda.warm-start-reuse lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after lambda.nodejs-v8-heap-exhaustion-aborts-distinct-from-os-level-out-of-memory-rejected lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot lambda.published-version-keeps-its-own-memory-limit-rejected-past-it lambda.python-runtime-defined-env-vars-match-the-runtime-identity lambda.python-user-env-vars-and-tz-override-reach-the-process lambda.ruby-reserved-env-vars-override-attempt lambda.runtime-update-reflected-in-sys-version lambda.stdlib-imports lambda.python-client-context-reaches-handler-as-typed-object lambda.python-context-version-and-invoked-arn-follow-the-qualifier lambda.python-invalid-client-context-rejected-before-the-handler-runs lambda.python-remaining-time-is-bounded-by-the-configured-timeout lambda.remaining-time-starts-at-the-configured-timeout-and-counts-down lambda.oversize-unhandled-error-becomes-response-size-too-large-rejected lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it lambda.python-builtin-module-as-handler-rejected lambda.python-dotted-handler-import-failures-name-the-dotted-module lambda.python-malformed-handler-name-rejected-at-invoke lambda.python-module-level-exception-rejected-with-its-own-error-type lambda.python-syntax-error-in-handler-module-rejected-at-invoke lambda.timed-out-invocation-releases-its-reserved-concurrency-slot lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it lambda.python-return-bytes-and-non-finite-floats-pass-through-unvalidated lambda.python-return-values-serialize-to-exact-json-bytes lambda.python-trailing-data-after-json-fails-runtime-unmarshal lambda.python-unparseable-payload-rejected-before-the-handler-runs lambda.nodejs-async-handler-never-settling-rejected-as-nodejs-exit lambda.nodejs-unknown-node-options-flag-rejected-as-exit-error
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.package-non-ascii-file-names-preserved-byte-exact|run#14|AssertionError|assert found == expected, (
Issue-Key: lambda.package-symlinks-extracted-as-symlinks-and-followed|_invoke_ok#10|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.python-handler-in-nested-package-resolves-relative-imports-and-files|_ok#6|AssertionError|assert r["status"] == 200 and r["error"] is None, (
Issue-Key: lambda.python-module-state-is-per-version-not-shared-with-latest|_ok#3|AssertionError|assert r["status"] == 200 and r["error"] is None, (
Issue-Key: lambda.python-runtime-family-every-supported-minor-runs-identically|_invoke_sync#5|AssertionError|raise AssertionError(
Issue-Key: lambda.task-root-and-runtime-dir-read-only-while-tmp-writable-rejected|_invoke_ok#9|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.tmp-not-shared-between-two-distinct-functions|_invoke_sync#9|AssertionError|raise AssertionError(
Issue-Key: lambda.tmp-persists-across-warm|_invoke_sync#5|AssertionError|raise AssertionError(
Issue-Key: lambda.tmp-writable|_invoke_sync#4|AssertionError|raise AssertionError(
Issue-Key: lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected|_invoke#6|AssertionError|assert "FunctionError" not in resp, f"handler failed with {resp.get('FunctionError')!r}: {raw!r}"
Issue-Key: lambda.package-file-modes-normalized-so-every-file-is-readable-and-executable|_invoke_ok#8|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.recursion-config-terminate-stops-a-self-invocation-chain|_run_chain#10|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.snapstart-python-version-restores-with-before-snapshot-state-and-a-restore-report|run#11|AssertionError|assert r["error"] is None, f"version invoke failed: {r['error']!r} {r['raw'][:400]!r}"
Issue-Key: lambda.snapstart-restored-version-reuses-the-snapshot-init|run#16|AssertionError|assert all(b["hook_import"] == "ok" for b in ver_bodies), (
Issue-Key: lambda.two-layers-sharing-a-path-resolve-to-the-last-layer-at-runtime|_invoke_json#12|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.nodejs-process-exit-reports-exit-error-and-reinitializes-rejected|run#9|AssertionError|assert kept["initId"] == first["initId"] and kept["calls"] == first["calls"] + 2, (
Issue-Key: lambda.nodejs-warm-environment-reuses-module-scope-and-tmp|run#7|AssertionError|assert max(len(c) for c in by_env.values()) >= 2, (
Issue-Key: lambda.out-of-memory-kill-discards-the-execution-environment|run#8|AssertionError|raise AssertionError(f"five invokes never reached environment {doomed!r}: {body!r}")
Issue-Key: lambda.provisioned-concurrency-invocation-reports-no-init-duration|_invoke_tail#6|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state|run#6|AssertionError|assert warm == {"calls": 2}, (
Issue-Key: lambda.stream-provisioned-environment-reports-provisioned-initialization-type|run#26|AssertionError|assert warm2["env_id"] == warm["env_id"], (
Issue-Key: lambda.warm-start-reuse|run#9|AssertionError|assert any(c > 1 for c in counts[1:]), (
Issue-Key: lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after|run#10|AssertionError|assert not ctl.get("FunctionError") and ctl_body == {"label": "control", "tagged": True}, (
Issue-Key: lambda.nodejs-v8-heap-exhaustion-aborts-distinct-from-os-level-out-of-memory-rejected|run#7|AssertionError|assert isinstance(limit, int) and limit < 2 * _OLD_SPACE_MB * 1024 * 1024, (
Issue-Key: lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot|run#12|AssertionError|assert detail[1].get("errorType") == "Runtime.OutOfMemory", f"400 MB body: {detail[1]!r}"
Issue-Key: lambda.published-version-keeps-its-own-memory-limit-rejected-past-it|_assert_out_of_memory#12|AssertionError|assert resp.get("FunctionError") == "Unhandled", (
Issue-Key: lambda.python-runtime-defined-env-vars-match-the-runtime-identity|run#3|AssertionError|assert got["env"] == want_env, (
Issue-Key: lambda.python-user-env-vars-and-tz-override-reach-the-process|run#9|AssertionError|assert got["tzname"] == ["EST", "EDT"], (
Issue-Key: lambda|CreateFunction|InvalidParameterValueException
Issue-Key: lambda.runtime-update-reflected-in-sys-version|run#5|AssertionError|assert _invoke(lam, fn) == ("$LATEST", {"python": "3.12"}), "python3.12 must run 3.12"
Issue-Key: lambda.stdlib-imports|run#7|AssertionError|assert resp["python_version"] == expected_version, (
Issue-Key: lambda.python-client-context-reaches-handler-as-typed-object|_ok#5|AssertionError|assert r["status"] == 200 and r["error"] is None, (
Issue-Key: lambda.python-context-version-and-invoked-arn-follow-the-qualifier|run#9|AssertionError|assert body["invoked_function_arn"] == want_arn, (
Issue-Key: lambda.python-invalid-client-context-rejected-before-the-handler-runs|_ok#5|AssertionError|assert r["status"] == 200 and r["error"] is None, (
Issue-Key: lambda.python-remaining-time-is-bounded-by-the-configured-timeout|run#4|AssertionError|assert 11000 < first <= 12000, (
Issue-Key: lambda.remaining-time-starts-at-the-configured-timeout-and-counts-down|_assert_deadline#5|AssertionError|assert isinstance(first, int) and budget - 1000 < first <= budget, (
Issue-Key: lambda.oversize-unhandled-error-becomes-response-size-too-large-rejected|run#12|AssertionError|assert body.get("errorType") == "Function.ResponseSizeTooLarge", (
Issue-Key: lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it|run#14|AssertionError|assert body.get("errorType") == "Sandbox.Timedout", f"version errorType: {body!r}"
Issue-Key: lambda.python-builtin-module-as-handler-rejected|run#6|AssertionError|assert body["errorType"] == "Runtime.BuiltInModuleConflict", f"errorType {body['errorType']!r}"
Issue-Key: lambda.python-dotted-handler-import-failures-name-the-dotted-module|_init_failure#3|AssertionError|assert body["requestId"] == "" and body["stackTrace"] == [], (
Issue-Key: lambda.python-malformed-handler-name-rejected-at-invoke|run#11|AssertionError|assert body["requestId"] == "" and body["stackTrace"] == [], (
Issue-Key: lambda.python-module-level-exception-rejected-with-its-own-error-type|run#5|AssertionError|assert body["errorType"] == "LookupError", (
Issue-Key: lambda.python-syntax-error-in-handler-module-rejected-at-invoke|run#4|AssertionError|assert body["errorType"] == "Runtime.UserCodeSyntaxError", (
Issue-Key: lambda.timed-out-invocation-releases-its-reserved-concurrency-slot|run#13|AssertionError|assert held[1][1].get("errorType") == "Sandbox.Timedout", f"overrun body: {held[1][1]!r}"
Issue-Key: lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it|_assert_timed_out#8|AssertionError|assert body.get("errorType") == "Sandbox.Timedout", f"{label}: errorType {body!r}"
Issue-Key: lambda.python-return-bytes-and-non-finite-floats-pass-through-unvalidated|run#7|AssertionError|assert raw["status"] == 200 and raw["error"] is None, (
Issue-Key: lambda.python-return-values-serialize-to-exact-json-bytes|run#3|AssertionError|assert r["error"] is None, (
Issue-Key: lambda|Invoke|InvalidRequestContentException
Issue-Key: lambda.python-unparseable-payload-rejected-before-the-handler-runs|run#5|AssertionError|assert msg == want, f"payload {payload!r}: message {msg!r}, expected {want!r}"
Issue-Key: lambda.nodejs-async-handler-never-settling-rejected-as-nodejs-exit|run#8|AssertionError|assert body["errorMessage"] == _MESSAGE, f"errorMessage was {body['errorMessage']!r}"
Issue-Key: lambda.nodejs-unknown-node-options-flag-rejected-as-exit-error|run#15|AssertionError|assert body == {
Issue-Commit: 76f746b5965121353b22ae227b995a601e952faf

# lambda: 10 net-new root causes in the managed execution environment (no sandbox filesystem, no environment reuse, MemorySize unenforced, wrong runtime identity, context/error-envelope shapes)

> _Note: this issue was identified against the code base at commit `76f746b5965121353b22ae227b995a601e952faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Ten distinct net-new root causes, all reached through Invoke against a python3.x/nodejs function. None of these checks is marked ALREADY FILED or OWNED in the replay report. Open issues 20261107T091747Z-lambda-the-execution-environment-has-no-filesystem-or.md and 20261107T091747Z-lambda-lambda-layers-are-never-materialized-into-the.md describe neighbouring symptoms but carry none of the replay keys below.

Cause 1 of 10: the invocation sandbox has no filesystem at all.
AWS: a Lambda invocation runs in a sandbox with /var/task (the unzipped package, read-only), /var/runtime (read-only) and a writable 512 MB /tmp that survives within one execution environment; os.listdir, open(), os.statvfs, os.readlink, os.path.getsize and platform.machine() all work. Smithy: CreateFunction.EphemeralStorage models the /tmp size; recorded behavior only for the filesystem layout itself.
Emulator: every such call fails the invocation with FunctionError='Unhandled' and errorType 'OSError', errorMessage either "a Lambda function in this emulator has no host filesystem or process to reach" or "a Lambda function in this emulator cannot open a file of the host". Both strings name the emulator and cannot occur on AWS.
Why: any handler that reads a packaged data file, writes scratch state to /tmp, or inspects the machine/architecture fails in the emulator and succeeds on AWS, so packaging and caching logic cannot be exercised at all.
Fix: the Lambda invoke runtime's sandbox/filesystem provisioning (materialise the unzipped package under /var/task and a per-environment writable /tmp) in the lambda service's invoke handler.
Traces: lambda.package-non-ascii-file-names-preserved-byte-exact, lambda.package-symlinks-extracted-as-symlinks-and-followed, lambda.python-handler-in-nested-package-resolves-relative-imports-and-files, lambda.python-module-state-is-per-version-not-shared-with-latest, lambda.python-runtime-family-every-supported-minor-runs-identically, lambda.task-root-and-runtime-dir-read-only-while-tmp-writable-rejected, lambda.tmp-not-shared-between-two-distinct-functions, lambda.tmp-persists-across-warm, lambda.tmp-writable.

Cause 2 of 10: the Python runtime's standard library and bundled packages are incomplete, so imports AWS resolves raise ImportModuleError/ModuleNotFoundError.
AWS: the managed python3.x runtimes ship a complete CPython stdlib (including the _signal, _socket and _ctypes extension modules), a bundled boto3/botocore (botocore.config importable), the snapshot_restore_py module on SnapStart-capable runtimes, and resolve modules supplied by attached layers from /opt/python.
Emulator: the handler is refused at import with errorType 'Runtime.ImportModuleError'/'ModuleNotFoundError' and messages "No module named '_signal'", "No module named '_socket'", "No module named 'botocore.config'", "No module named 'snapshot_restore_py'" and "No module named 'only_a'" (a module published in an attached layer).
Why: a client's handler that imports signal/socket/ctypes, configures botocore, registers SnapStart restore hooks, or depends on a layer-supplied package is reported as a broken deployment package when AWS would run it.
Fix: the Python runtime image/module resolution used by the invoke runtime (stdlib extension modules, bundled botocore, snapshot_restore_py, and /opt/python on sys.path).
Traces: lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected, lambda.package-file-modes-normalized-so-every-file-is-readable-and-executable, lambda.recursion-config-terminate-stops-a-self-invocation-chain, lambda.snapstart-python-version-restores-with-before-snapshot-state-and-a-restore-report, lambda.snapstart-restored-version-reuses-the-snapshot-init, lambda.two-layers-sharing-a-path-resolve-to-the-last-layer-at-runtime.

Cause 3 of 10: no execution environment is ever reused — every invocation is a cold start.
AWS: consecutive invocations of an unchanged function reuse one execution environment: module-level state and the init-time environment id persist, a cold start happens only after a configuration/code change, a Runtime.ExitError, or an out-of-memory kill, and provisioned concurrency serves pre-warmed environments.
Emulator: back-to-back invokes each report a fresh environment id and a module-level counter of 1 ('four sequential invokes never reused an environment: counters per environment [1, 1, 1, 1]'; 'no warm-start reuse observed across 5 sequential invocations: counters=[1, 1, 1, 1, 1, 1]'), and a provisioned-concurrency invoke returns a second new id or fails with "<requestId> Task timed out after 30.00 seconds".
Why: warm-start caching, connection reuse, provisioned-concurrency behaviour and every 'did this reset the environment' distinction are unobservable, and handlers that memoise clients cannot be validated.
Fix: the execution-environment lifecycle/pool in the invoke runtime (keep an environment keyed by function version+config and only discard it on update, exit or OOM).
Traces: lambda.nodejs-process-exit-reports-exit-error-and-reinitializes-rejected, lambda.nodejs-warm-environment-reuses-module-scope-and-tmp, lambda.out-of-memory-kill-discards-the-execution-environment, lambda.provisioned-concurrency-invocation-reports-no-init-duration, lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state, lambda.stream-provisioned-environment-reports-provisioned-initialization-type, lambda.warm-start-reuse.

Cause 4 of 10: MemorySize is reported but never enforced — there is no out-of-memory kill.
AWS: Smithy models CreateFunction.MemorySize @range min 128 max 32768; the sandbox is capped at that value, so a handler that allocates past it is killed and Invoke answers StatusCode 200 with FunctionError 'Unhandled' and an errorMessage naming the memory limit; a node runtime's V8 heap is bounded by it (and by --max-old-space-size in NODE_OPTIONS).
Emulator: a handler holding 400 MB under MemorySize=128 returns FunctionError=None and reports memory_limit_in_mb='128' while still holding the block, or the invocation instead hits the function timeout ("<requestId> Task timed out after 30.00 seconds"); v8.getHeapStatistics().heap_size_limit comes back None.
Why: memory sizing, OOM retry/dead-letter behaviour and cost/limit tuning cannot be exercised; a client's OOM path silently turns into a timeout path with a different error envelope and a different retry profile.
Fix: memory accounting/enforcement in the invoke runtime's sandbox limits, including the node heap flags.
Traces: lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after, lambda.nodejs-v8-heap-exhaustion-aborts-distinct-from-os-level-out-of-memory-rejected, lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot, lambda.published-version-keeps-its-own-memory-limit-rejected-past-it.

Cause 5 of 10: the runtime identity does not follow the configured Runtime, and reserved/user environment variables are mishandled.
AWS: a function configured Runtime=python3.12 runs CPython 3.12 (sys.version_info[:2] == (3, 12)); the runtime-defined environment contains PYTHONPATH=/var/runtime; a user-supplied TZ (e.g. America/New_York) reaches the process so time.tzname is ('EST', 'EDT'); the reserved-key set CreateFunction refuses is AWS's documented list and the refusal is InvalidParameterValueException.
Emulator: the handler reports sys.version_info [3, 14] whatever Runtime is configured ('python3.12 must run 3.12'), PYTHONPATH is None, time.tzname stays ['UTC', 'UTC'] under TZ=America/New_York, and CreateFunction refuses with InvalidParameterValueException "Lambda was unable to configure your environment variables because the environment variables you have provided contains reserved keys that are currently not supported for modification. Reserved keys used in this request: AWS_EXECUTION_ENV" where the check expected the call to proceed.
Why: version-sensitive code, timezone-dependent formatting and PYTHONPATH-dependent imports behave differently in the emulator than on the configured runtime, and the reserved-key contract diverges at create time.
Fix: runtime selection and the runtime-defined/user environment assembly in the invoke runtime, plus the reserved-key validation in the CreateFunction handler.
Traces: lambda.python-runtime-defined-env-vars-match-the-runtime-identity, lambda.python-user-env-vars-and-tz-override-reach-the-process, lambda.ruby-reserved-env-vars-override-attempt, lambda.runtime-update-reflected-in-sys-version, lambda.stdlib-imports.

Cause 6 of 10: the context object handed to the handler has the wrong shape and its remaining-time clock starts late.
AWS: context.identity is an object exposing cognito_identity_id/cognito_identity_pool_id (None when absent) and context.client_context is an object with .custom/.env/.client, not a dict; context.invoked_function_arn carries the invoked qualifier, so an unqualified invoke ends in ':$LATEST'; get_remaining_time_in_millis() at handler entry is just under the configured Timeout (Smithy: CreateFunction.Timeout @range min 1 max 5400, in seconds).
Emulator: ident.cognito_identity_id raises AttributeError ("'NoneType' object has no attribute 'cognito_identity_id'"), cc.custom raises AttributeError ("'dict' object has no attribute 'custom'"), invoked_function_arn has suffix '' instead of ':$LATEST', and the first reading is 2967 ms for Timeout=4 and 10859 ms for Timeout=12 — i.e. the deadline clock starts well before the handler is entered.
Why: any handler that reads identity/client context through the documented attribute interface crashes, ARN-qualifier-driven logic takes the wrong branch, and self-imposed deadlines computed from remaining time are short by hundreds of milliseconds.
Fix: context construction and the invocation deadline in the invoke runtime's handler bootstrap.
Traces: lambda.python-client-context-reaches-handler-as-typed-object, lambda.python-context-version-and-invoked-arn-follow-the-qualifier, lambda.python-invalid-client-context-rejected-before-the-handler-runs, lambda.python-remaining-time-is-bounded-by-the-configured-timeout, lambda.remaining-time-starts-at-the-configured-timeout-and-counts-down.

Cause 7 of 10: the function-error envelope carries the wrong errorType and spurious requestId/stackTrace fields.
AWS: when the runtime cannot load the handler, the envelope is {errorMessage, errorType, stackTrace} with errorType fixed per failure class — 'Runtime.HandlerNotFound' for a handler string that names no loadable function, the module's own exception class for an exception raised at module import (e.g. 'LookupError'), 'Runtime.ImportModuleError' naming the dotted module for an import failure — and an init-time failure carries no requestId and an empty stackTrace; a timeout answers FunctionError 'Unhandled' with errorMessage '<requestId> Task timed out after <N>.00 seconds'; a handler error whose serialized payload exceeds 6 MB is replaced by a response-size refusal rather than being returned verbatim.
Emulator: errorType is collapsed to 'Runtime.ImportModuleError' for a module-level exception and for a syntax error, a builtin-module handler reports 'Runtime.HandlerNotFound' where the check expects a different class, init-time envelopes carry a populated requestId ('2550f0bb-…') instead of '' , the timeout envelope omits the modelled errorType, and a 7340032-byte handler error is returned with errorType 'Sim2RealError' and a 7 MB message.
Why: clients that branch on errorType, or that rely on the 6 MB response cap, mis-classify failures; an emulator-only errorType ('Sim2RealError') cannot occur on AWS.
Fix: the error-envelope serializer and the response-size cap in the invoke runtime.
Traces: lambda.oversize-unhandled-error-becomes-response-size-too-large-rejected, lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it, lambda.python-builtin-module-as-handler-rejected, lambda.python-dotted-handler-import-failures-name-the-dotted-module, lambda.python-malformed-handler-name-rejected-at-invoke, lambda.python-module-level-exception-rejected-with-its-own-error-type, lambda.python-syntax-error-in-handler-module-rejected-at-invoke, lambda.timed-out-invocation-releases-its-reserved-concurrency-slot, lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it.

Cause 8 of 10: the response marshaller rejects values the managed Python runtime passes through.
AWS: the python runtime's serializer accepts bytes and non-finite floats and decimal values the handler returns, producing exact JSON bytes; Runtime.MarshalError is reserved for values it genuinely cannot encode.
Emulator: returning bytes answers FunctionError 'Unhandled' errorType 'Runtime.MarshalError' message "Unable to marshal response: Object of type bytes is not JSON serializable", and returning decimal.Decimal('1.10') answers the same error with "Object of type Decimal is not JSON serializable".
Why: handlers whose return values AWS serialises are reported as failed invocations, so response-shape contracts cannot be validated.
Fix: the response serializer in the Python invoke runtime.
Traces: lambda.python-return-bytes-and-non-finite-floats-pass-through-unvalidated, lambda.python-return-values-serialize-to-exact-json-bytes.

Cause 9 of 10: unparseable Invoke payloads are refused with the wrong message, and trailing data is surfaced as a client exception instead of a runtime unmarshal error.
AWS: Smithy models Invoke error InvalidRequestContentException (httpError 400). For a whitespace-only payload the message is 'Could not parse request body into json: Could not parse payload into json: No payload content found'; the fixed text is that sentence and only the parser's position detail varies. A payload with valid JSON followed by trailing data reaches the runtime and fails as a runtime unmarshal error rather than as a 400 to the caller.
Emulator: the whitespace payload is refused with 'Could not parse request body into json: Could not parse payload into json: Unexpected end-of-input: expected a value\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 3]', and the trailing-data payload raises InvalidRequestContentException to the caller with "Unexpected character ('x' (code 120)): was expecting end of the document".
Why: a client matching on the documented refusal text, or expecting the runtime (not the control path) to fail on trailing data, sees a different error class and message.
Fix: payload validation in the Invoke handler and the hand-off of the raw payload to the runtime.
Traces: lambda.python-trailing-data-after-json-fails-runtime-unmarshal, lambda.python-unparseable-payload-rejected-before-the-handler-runs.

Cause 10 of 10: the Node.js runtime's exit/flag failures are reported with the wrong envelope.
AWS: a node handler that never settles its promise is reported as a Runtime.ExitError-class failure whose errorMessage is AWS's fixed text for that class, and an unknown NODE_OPTIONS flag fails the init with the node bootstrap's own message, not as an invocation-level exit error.
Emulator: the never-settling handler answers errorMessage 'Node.js process exited before the invocation was answered' (emulator-authored prose), and the unknown flag answers errorType 'Runtime.NodeJsExit' with errorMessage 'node: bad option: --sim2real-no-such-node-flag' (the flag name is the variable part taken from the request; the surrounding classification is wrong).
Why: node clients that distinguish an exited process from a timed-out or unsettled invocation key off errorType/errorMessage and mis-classify both cases.
Fix: the Node.js runtime adapter's exit/init error mapping in the invoke runtime.
Traces: lambda.nodejs-async-handler-never-settling-rejected-as-nodejs-exit, lambda.nodejs-unknown-node-options-flag-rejected-as-exit-error.

## Evidence
Cause 1: FunctionError='Unhandled' {"errorType":"OSError","errorMessage":"a Lambda function in this emulator has no host filesystem or process to reach"} for os.listdir('/tmp'), os.statvfs('/tmp'), os.readlink, os.path.getsize('/opt/blob/zeros.bin') and platform.machine(); expected a clean 200 with the filesystem report. Non-ASCII package entries: expected ['données/résumé.txt', 'emoji/🚀-launch.txt', 'mixed/Ünïcødé ✓ name.txt', '日本語/ファイル.txt'], guest saw [].
Cause 2: {"errorType":"Runtime.ImportModuleError","errorMessage":"Unable to import module 'index': No module named '_signal'"}; likewise '_socket', 'botocore.config', 'snapshot_restore_py', and ModuleNotFoundError "No module named 'only_a'" for a layer module.
Cause 3: 'four sequential invokes never reused an environment: counters per environment [1, 1, 1, 1]'; 'no warm-start reuse observed across 5 sequential invocations: counters=[1, 1, 1, 1, 1, 1]'; 'a second sequential invocation must reuse the one provisioned environment, got 1b22d010-… after f64bc184-…'.
Cause 4: '400 MB at 128 MB: FunctionError=None, expected Unhandled: {held_mb: 400, memory_limit_in_mb: 128}'; '--max-old-space-size=256 must bound V8's heap far below MemorySize=1536; heap_size_limit was None'.
Cause 5: 'runtime=python3.12 declares sys.version_info[:2]==[3, 12], the handler reported [3, 14]'; PYTHONPATH None vs '/var/runtime'; 'with TZ=America/New_York time.tzname is [UTC, UTC], expected [EST, EDT]'.
Cause 6: AttributeError "'NoneType' object has no attribute 'cognito_identity_id'"; AttributeError "'dict' object has no attribute 'custom'"; "$LATEST: invoked_function_arn has suffix '', expected ':$LATEST'"; 'with Timeout=12 the first reading was 10859 ms, expected in (11000, 12000]'.
Cause 7: "attempt 1: errorType 'Runtime.ImportModuleError', expected 'LookupError'"; "requestId/stackTrace '2550f0bb-4093-4c4f-9e84-65d6b9863587'/[], expected ''/[]"; "huge error errorType 'Sim2RealError'; message length 7340032".
Cause 8: {"errorType":"Runtime.MarshalError","errorMessage":"Unable to marshal response: Object of type bytes is not JSON serializable"} and the same for Decimal.
Cause 9: InvalidRequestContentException(400) "Could not parse request body into json: Could not parse payload into json: Unexpected character ('x' (code 120)): was expecting end of the document" raised to the caller; whitespace payload message ends '…Unexpected end-of-input: expected a value\n at [Source: REDACTED …; line: 1, column: 3]' instead of '…No payload content found'.
Cause 10: errorMessage 'Node.js process exited before the invocation was answered'; errorType 'Runtime.NodeJsExit' errorMessage 'node: bad option: --sim2real-no-such-node-flag'.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. Re-replay the check ids in trace_ids; they are all listed under '### lambda' in the report with verdict [mismatch] or [error].
3. Minimal repro for cause 1: CreateFunction(Runtime='python3.12', Handler='index.handler') with a handler that returns sorted(os.listdir('/tmp')), then Invoke — the emulator answers 200 with FunctionError='Unhandled' and errorType 'OSError'.
4. Minimal repro for cause 3: Invoke the same function twice back-to-back and compare a module-level counter — it is 1 both times.
5. Minimal repro for cause 5: Invoke a python3.12 function returning sys.version_info[:2] — it reports (3, 14).
