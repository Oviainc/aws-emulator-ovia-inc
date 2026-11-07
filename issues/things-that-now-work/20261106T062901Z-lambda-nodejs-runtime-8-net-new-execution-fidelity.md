Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.nodejs-runtime-executes lambda.nodejs-runtime-family-matrix-identical-across-every-supported-identifier lambda.nodejs-warm-environment-reuses-module-scope-and-tmp lambda.nodejs-filesystem-outside-tmp-is-read-only-rejected lambda.nodejs-require-in-type-module-package-rejected lambda.nodejs-mjs-handler-top-level-await-completes-during-init lambda.stream-handler-throw-delivers-no-bytes-and-no-error-code lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment lambda.stream-response-chunks-map-onto-handler-writes lambda.layer-nodejs-other-major-directory-not-resolvable-rejected lambda.nodejs-builtin-sdk-v3-clients-resolve-without-bundling lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes lambda.layer-nodejs-function-node-modules-shadows-layer-package lambda.layer-nodejs-node-modules-paths-resolve-by-require lambda.nodejs-unicode-payload-round-trips lambda.nodejs-tz-environment-variable-sets-local-time-zone lambda.nodejs-runtime-environment-variables-identify-runtime lambda.nodejs-rejected-promise-reports-rejection-reason-rejected lambda.nodejs-thrown-error-reports-type-message-and-trace-rejected lambda.nodejs-thrown-non-error-values-report-typeof-rejected lambda.nodejs-callback-error-reports-unhandled-rejected lambda.nodejs-init-exception-reported-on-invoke-rejected lambda.nodejs-callback-and-context-succeed-done-return-result lambda.nodejs-esm-syntax-in-commonjs-js-rejected lambda.nodejs-stray-unhandled-rejection-fails-invoke-rejected lambda.nodejs-unknown-node-options-flag-rejected-as-exit-error lambda.nodejs-async-handler-never-settling-rejected-as-nodejs-exit lambda.nodejs-handler-in-nested-directory-resolves lambda.nodejs-js-file-wins-over-mjs-for-same-handler-module lambda.nodejs-package-type-module-loads-js-as-esm lambda.nodejs-node-options-preloads-module-and-sets-flags
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.nodejs-runtime-executes|_invoke_sync#12|AssertionError|raise AssertionError(
Issue-Key: lambda.nodejs-runtime-family-matrix-identical-across-every-supported-identifier|_invoke#6|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.nodejs-warm-environment-reuses-module-scope-and-tmp|_ok#3|AssertionError|assert err is None, f"Invoke {name}: FunctionError={err!r}, payload {raw[:600]!r}"
Issue-Key: lambda.nodejs-filesystem-outside-tmp-is-read-only-rejected|_invoke#4|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.nodejs-require-in-type-module-package-rejected|_ok#5|AssertionError|assert err is None, f"Invoke {name}: FunctionError={err!r}, payload {raw[:600]!r}"
Issue-Key: lambda.nodejs-mjs-handler-top-level-await-completes-during-init|_ok#6|AssertionError|assert err is None, f"Invoke {name}: FunctionError={err!r}, payload {raw[:600]!r}"
Issue-Key: lambda.stream-handler-throw-delivers-no-bytes-and-no-error-code|run#7|AssertionError|assert len(ok_frames) >= 2, (
Issue-Key: lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment|_stream#10|AssertionError|assert "ErrorCode" not in complete, (
Issue-Key: lambda.stream-response-chunks-map-onto-handler-writes|_drain#10|AssertionError|assert expected.startswith(acc), (
Issue-Key: lambda.layer-nodejs-other-major-directory-not-resolvable-rejected|_invoke_ok#8|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.nodejs-builtin-sdk-v3-clients-resolve-without-bundling|_ok#3|AssertionError|assert err is None, f"Invoke {name}: FunctionError={err!r}, payload {raw[:600]!r}"
Issue-Key: lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes|run#10|AssertionError|assert rest == b"", f"stream ended mid-line: trailing bytes {rest[:200]!r}"
Issue-Key: lambda.layer-nodejs-function-node-modules-shadows-layer-package|_invoke_ok#14|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-nodejs-node-modules-paths-resolve-by-require|_invoke_ok#9|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.nodejs-unicode-payload-round-trips|run#5|AssertionError|assert err is None, f"FunctionError={err!r}, payload {raw!r}"
Issue-Key: lambda.nodejs-tz-environment-variable-sets-local-time-zone|_ok#8|AssertionError|assert err is None, f"Invoke {name}: FunctionError={err!r}, payload {raw[:600]!r}"
Issue-Key: lambda.nodejs-runtime-environment-variables-identify-runtime|_ok#4|AssertionError|assert err is None, f"Invoke {name}: FunctionError={err!r}, payload {raw[:600]!r}"
Issue-Key: lambda.nodejs-rejected-promise-reports-rejection-reason-rejected|run#7|AssertionError|assert sorted(body) == ["errorMessage", "errorType", "trace"], (
Issue-Key: lambda.nodejs-thrown-error-reports-type-message-and-trace-rejected|run#6|AssertionError|assert sorted(body) == ["errorMessage", "errorType", "trace"], (
Issue-Key: lambda.nodejs-thrown-non-error-values-report-typeof-rejected|run#7|AssertionError|assert body == {"errorType": "string", "errorMessage": f"thrown text {rid}", "trace": []}, (
Issue-Key: lambda.nodejs-callback-error-reports-unhandled-rejected|run#9|AssertionError|assert body["trace"][0] == f"RangeError: out of range {rid}", f"trace head: {body['trace']!r}"
Issue-Key: lambda.nodejs-init-exception-reported-on-invoke-rejected|run#10|AssertionError|assert body["trace"][0] == f"Error: init exploded: {mark}", f"trace head {body['trace'][:1]!r}"
Issue-Key: lambda.nodejs-callback-and-context-succeed-done-return-result|_ok#6|AssertionError|assert err is None, f"Invoke {name}: FunctionError={err!r}, payload {raw[:600]!r}"
Issue-Key: lambda.nodejs-esm-syntax-in-commonjs-js-rejected|_unhandled#9|AssertionError|assert err == "Unhandled", f"Invoke {name}: FunctionError={err!r}, expected 'Unhandled'; payload {raw[:600]!r}"
Issue-Key: lambda.nodejs-stray-unhandled-rejection-fails-invoke-rejected|_unhandled#6|AssertionError|assert err == "Unhandled", f"Invoke {name}: FunctionError={err!r}, expected 'Unhandled'; payload {raw[:600]!r}"
Issue-Key: lambda.nodejs-unknown-node-options-flag-rejected-as-exit-error|run#12|AssertionError|assert resp.get("FunctionError") == "Unhandled", f"FunctionError {resp.get('FunctionError')!r}"
Issue-Key: lambda.nodejs-async-handler-never-settling-rejected-as-nodejs-exit|run#7|KeyError|assert body["errorType"] == "Runtime.NodeJsExit", f"a never-settling handler reported {body!r}"
Issue-Key: lambda.nodejs-handler-in-nested-directory-resolves|run#9|AssertionError|assert got == {"value": v * 5, "file": "file:///var/task/src/handlers/api.mjs"}, f"nested ES module handler: {got!r}"
Issue-Key: lambda.nodejs-js-file-wins-over-mjs-for-same-handler-module|run#9|AssertionError|assert got == {"loaded": "index.mjs", "file": "file:///var/task/index.mjs", "id": rid}, (
Issue-Key: lambda.nodejs-package-type-module-loads-js-as-esm|run#6|AssertionError|assert got == {"kind": "esm", "requireType": "undefined", "url": "file:///var/task/index.js", "n": n + 1}, (
Issue-Key: lambda.nodejs-node-options-preloads-module-and-sets-flags|run#8|AssertionError|assert got == {"preloaded": f"preload:{mark}", "stackTraceLimit": 3, "nodeOptions": options}, (
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# Lambda nodejs runtime: 8 net-new execution-fidelity causes (no core modules, no bundled SDK v3, no node_modules resolution, missing globals, wrong error payload, callback/context API, unreported failures, ESM/NODE_OPTIONS)

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
The emulator runs nodejs2x functions in an in-process JS interpreter (stack frames name `eval_script` and `<input>`), and that interpreter diverges from the AWS managed Node.js runtime in 8 distinct ways. 8 causes are bundled here; all are reached through Invoke on a nodejs22.x/nodejs20.x function. An open issue (20261105T080515Z-dynamodb-lambda-rekognition-s3-lambda-invoke-refuses-a-nodejs22-x-function-with.md) covers Invoke REFUSING some runtimes; none of the causes below are covered by it - here Invoke succeeds and the handler misbehaves.

Cause 1 of 8: Node core modules are not resolvable.
AWS: Smithy: Invoke POST /2015-03-31/functions/{FunctionName}/invocations, output FunctionError in header X-Amz-Function-Error. On a managed nodejs runtime `require('fs')`, `require('crypto')`, `require('timers/promises')` and `import 'node:crypto'` resolve from the runtime itself, so a handler that uses them returns 200 with no X-Amz-Function-Error. Recorded behavior only for the module list.
Emulator: the invoke returns 200 with FunctionError='Unhandled' and payload {"errorMessage":"Error: Cannot find module 'fs'","errorType":"Runtime.ImportModuleError",...}; same for 'crypto', 'timers/promises', and ReferenceError "Error resolving module 'node:crypto' from 'index.mjs'".
Why: no Node handler that touches the standard library can run, so every nodejs scenario (including response streaming, which uses timers/promises) reports a handler failure a client must treat as its own bug.
Fix: the node runtime module behind the Invoke handler of the lambda service crate - it must expose the Node core module set to require()/import.
Traces: lambda.nodejs-runtime-executes, lambda.nodejs-runtime-family-matrix-identical-across-every-supported-identifier, lambda.nodejs-warm-environment-reuses-module-scope-and-tmp, lambda.nodejs-filesystem-outside-tmp-is-read-only-rejected, lambda.nodejs-require-in-type-module-package-rejected, lambda.nodejs-mjs-handler-top-level-await-completes-during-init, lambda.stream-handler-throw-delivers-no-bytes-and-no-error-code, lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment, lambda.stream-response-chunks-map-onto-handler-writes, lambda.layer-nodejs-other-major-directory-not-resolvable-rejected.

Cause 2 of 8: the bundled AWS SDK for JavaScript v3 is absent.
AWS: on nodejs18.x and later the runtime ships @aws-sdk/client-* so `require('@aws-sdk/client-s3')` resolves without bundling; the invoke returns 200 and no FunctionError. Recorded behavior only.
Emulator: FunctionError='Unhandled', payload errorMessage "Cannot find module '@aws-sdk/client-s3'" / "Error: Cannot find module '@aws-sdk/client-lambda'", errorType Error / Runtime.ImportModuleError.
Why: the single most common Node handler shape (call another AWS API) cannot run at all.
Fix: the node runtime's module registry - preload the v3 client packages the managed runtime bundles.
Traces: lambda.nodejs-builtin-sdk-v3-clients-resolve-without-bundling, lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes.

Cause 3 of 8: node_modules shipped in the package or in a layer is not on the resolver path.
AWS: require('<pkg>') resolves from /var/task/node_modules first and then /opt/nodejs/node_modules (layer), so the function's own copy shadows the layer's; both invokes return a value, not an error. Recorded behavior only.
Emulator: both invokes report FunctionError='Unhandled' with errorMessage "Cannot find module '<pkg>'", errorType Error, and frames at `moduleNotFound (eval_script:...)`.
Why: layers and vendored dependencies are unusable from Node handlers, and the shadowing precedence can never be observed.
Fix: the node runtime's module resolver - add /var/task/node_modules and the layer /opt/nodejs(/node14|node_modules) search path.
Traces: lambda.layer-nodejs-function-node-modules-shadows-layer-package, lambda.layer-nodejs-node-modules-paths-resolve-by-require.

Cause 4 of 8: runtime globals are missing (Buffer, Intl, process.versions).
AWS: a managed Node runtime defines the Buffer global, a full-ICU Intl, and process.versions.node; handlers reading them return 200 with no FunctionError. Recorded behavior only.
Emulator: FunctionError='Unhandled' with ReferenceError "Buffer is not defined", ReferenceError "Intl is not defined" and TypeError "cannot read property 'node' of undefined".
Why: binary payload handling, TZ/locale-sensitive code and any runtime-identity reporting fail inside the handler.
Fix: the node runtime's global environment setup.
Traces: lambda.nodejs-unicode-payload-round-trips, lambda.nodejs-tz-environment-variable-sets-local-time-zone, lambda.nodejs-runtime-environment-variables-identify-runtime.

Cause 5 of 8: the Node error payload carries an extra requestId key and a headless trace.
AWS: for a nodejs runtime the error payload is exactly {errorType, errorMessage, trace} - the request id is carried in the response, not in the body - and trace[0] is the "<ErrorType>: <message>" line, followed by "    at ..." frames of the user's file. The fixed parts are the three key names and the head line's shape; the type, message and frame paths come from the thrown value and the package.
Emulator: the payload keys are ['errorMessage','errorType','requestId','trace'] and trace either is empty or starts directly with a frame naming the interpreter ("    at <anonymous> (<input>:6:80)", "    at callHandler (eval_script:662:46)").
Why: clients that parse the documented three-key payload, or read trace[0] to recover the error class and message, mis-parse every Node failure.
Fix: the Invoke handler's function-error serializer for the node runtime.
Traces: lambda.nodejs-rejected-promise-reports-rejection-reason-rejected, lambda.nodejs-thrown-error-reports-type-message-and-trace-rejected, lambda.nodejs-thrown-non-error-values-report-typeof-rejected, lambda.nodejs-callback-error-reports-unhandled-rejected, lambda.nodejs-init-exception-reported-on-invoke-rejected.

Cause 6 of 8: the legacy callback/context completion API is not provided.
AWS: the Node handler's context exposes succeed/fail/done as functions as well as the (err, result) callback; using any of them returns the result with no FunctionError. Recorded behavior only.
Emulator: FunctionError='Unhandled' with TypeError "not a function" raised at the handler line that calls context.succeed/context.done.
Why: handlers written against the callback/context style fail instead of returning.
Fix: the node runtime's handler invocation shim (context object construction).
Traces: lambda.nodejs-callback-and-context-succeed-done-return-result.

Cause 7 of 8: invocations that AWS fails are reported as successes.
AWS: ESM syntax in a CommonJS .js file, a stray unhandled promise rejection, an unknown NODE_OPTIONS flag and a handler that never settles each make the invoke report X-Amz-Function-Error: Unhandled (errorType Runtime.NodeJsExit for the never-settling/exit cases). Recorded behavior only.
Emulator: the same invokes return 200 with no FunctionError and the handler's own payload (e.g. b'{"ran":"r-..."}', b'{"done":"r-..."}'), or a body with no errorType at all.
Why: a negative control passes, so a client cannot tell a failed invocation from a successful one and retry/alarm logic never fires.
Fix: the node runtime's invoke lifecycle - detect module-load syntax errors, unhandled rejections, bad runtime flags and non-settling handlers and surface them as function errors.
Traces: lambda.nodejs-esm-syntax-in-commonjs-js-rejected, lambda.nodejs-stray-unhandled-rejection-fails-invoke-rejected, lambda.nodejs-unknown-node-options-flag-rejected-as-exit-error, lambda.nodejs-async-handler-never-settling-rejected-as-nodejs-exit.

Cause 8 of 8: ESM module identity and NODE_OPTIONS are ignored.
AWS: an ES module handler sees import.meta.url as file:///var/task/<path to the loaded module>, `index.js` under package type=module loads as ESM with that url, a .js module wins over a same-named .mjs, and NODE_OPTIONS is applied (--require preloads a module, --stack-trace-limit sets Error.stackTraceLimit). Recorded behavior only.
Emulator: the handler's reported object omits the file/url field entirely ({'value': 130} with no 'file'; {'kind':'esm','requireType':'undefined'} with no 'url'), and with NODE_OPTIONS='--require /var/task/preload.js --stack-trace-limit=3' the handler reports preloaded=None and stackTraceLimit=10.
Why: ESM handlers that resolve sibling files from import.meta.url break, and the documented NODE_OPTIONS knob silently does nothing.
Fix: the node runtime's module loader (module url) and its process-environment/option plumbing.
Traces: lambda.nodejs-handler-in-nested-directory-resolves, lambda.nodejs-js-file-wins-over-mjs-for-same-handler-module, lambda.nodejs-package-type-module-loads-js-as-esm, lambda.nodejs-node-options-preloads-module-and-sets-flags.

## Evidence
Expected (AWS): Invoke of a nodejs22.x function returns 200 with no X-Amz-Function-Error; require('fs')/require('crypto')/require('@aws-sdk/client-s3')/require('<pkg from node_modules>') resolve; Buffer/Intl/process.versions.node exist; the function-error body has exactly {errorType, errorMessage, trace} with trace[0] = "<ErrorType>: <message>"; context.succeed/fail/done are functions; ESM syntax in .js, stray unhandled rejections, bad NODE_OPTIONS flags and non-settling handlers report FunctionError 'Unhandled'; import.meta.url = file:///var/task/<module>; NODE_OPTIONS --require/--stack-trace-limit take effect.
Actual (emulator): 200 + FunctionError='Unhandled' with {"errorMessage":"Error: Cannot find module 'fs'","errorType":"Runtime.ImportModuleError","requestId":"...","trace":[]}; "Cannot find module '@aws-sdk/client-s3'"; "Cannot find module '<pkg>'" with frames at moduleNotFound (eval_script:184:66); ReferenceError "Buffer is not defined" / "Intl is not defined"; TypeError "cannot read property 'node' of undefined"; error payload keys ['errorMessage','errorType','requestId','trace'] and trace heads that are interpreter frames (<input>, eval_script); TypeError "not a function" for context.succeed/done; FunctionError=None for the four must-fail cases; handler result objects missing their file/url field; preloaded=None, stackTraceLimit=10.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the 31 check ids in trace_ids (all under service heading `lambda`).
2. Minimal repro per cause: CreateFunction with Runtime=nodejs22.x, Handler=index.handler and a zip whose index.js does (a) require('fs'), (b) require('@aws-sdk/client-s3'), (c) require a package placed in node_modules/ of the zip, (d) return {b: typeof Buffer, i: typeof Intl, v: process.versions.node}, (e) throw new RangeError('x'), (f) call context.succeed({}), (g) use ESM `import` syntax in index.js, (h) export an async handler that never resolves, (i) an .mjs handler returning import.meta.url, (j) set Environment NODE_OPTIONS='--require /var/task/preload.js --stack-trace-limit=3'.
3. Invoke each and read X-Amz-Function-Error and the payload.
