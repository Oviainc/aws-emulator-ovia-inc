Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.layer-code-importable lambda.layer-count-over-five-rejected lambda.layer-declared-incompatible-runtime-and-architecture-still-attaches-and-loads lambda.layer-published-version-keeps-its-layer-after-latest-repoints-and-layer-deleted lambda.layers-deleted-version-still-resolves-new-reference-rejected lambda.layers-detach-breaks-handler-import lambda.layers-version-pin-ignores-newer-version lambda.layers-toplevel-zip-entry-not-importable-rejected lambda.layer-unchanged-reattach-of-a-deleted-version-still-revalidated lambda.layer-python-site-packages-dir-importable-and-precedes-python-root lambda.layer-python-site-packages-for-other-minor-version-not-importable-rejected lambda.layer-pth-files-in-layer-site-dirs-extend-sys-path lambda.layers-function-package-shadows-layer-module lambda.layer-lib-dir-on-ld-library-path-loads-shared-object-by-bare-name lambda.layers-bin-on-path-executes-layer-binary lambda.layers-combined-unzipped-size-over-250mb-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.layer-code-importable|_invoke_sync#12|AssertionError|raise AssertionError(
Issue-Key: lambda.layer-count-over-five-rejected|run#11|AssertionError|assert "FunctionError" not in resp, resp["Payload"].read()
Issue-Key: lambda.layer-declared-incompatible-runtime-and-architecture-still-attaches-and-loads|_invoke_ok#13|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-published-version-keeps-its-layer-after-latest-repoints-and-layer-deleted|_invoke_ok#20|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layers-deleted-version-still-resolves-new-reference-rejected|_invoke_json#12|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-detach-breaks-handler-import|_invoke_json#11|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-version-pin-ignores-newer-version|_invoke_json#16|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-toplevel-zip-entry-not-importable-rejected|_invoke_json#13|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layer-unchanged-reattach-of-a-deleted-version-still-revalidated|run#7|AssertionError|assert _invoke(lam, fn) == ({"marker": token}, None), "positive control: the layer module must import"
Issue-Key: lambda.layer-python-site-packages-dir-importable-and-precedes-python-root|_invoke_ok#9|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-python-site-packages-for-other-minor-version-not-importable-rejected|_invoke_ok#8|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-pth-files-in-layer-site-dirs-extend-sys-path|_invoke_ok#9|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layers-function-package-shadows-layer-module|run#19|AssertionError|assert "/var/task" in syspath and "/opt/python" in syspath, (
Issue-Key: lambda.layer-lib-dir-on-ld-library-path-loads-shared-object-by-bare-name|_invoke_ok#10|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layers-bin-on-path-executes-layer-binary|_invoke_json#11|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-combined-unzipped-size-over-250mb-rejected|_invoke_ok#10|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Commit: 677b6e35c5226b67b9bfea5e554d250e3c084a9c

# lambda: Lambda layers are never materialized into the execution environment — /opt is empty, sys.path omits /var/task and /opt (6 net-new causes)

> _Note: this issue was identified against the code base at commit `677b6e35c5226b67b9bfea5e554d250e3c084a9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Six net-new root causes, all in how the Lambda data plane builds the execution environment for a function that has Layers attached. Related open issues name the Python runtime's missing AWS SDK (e.g. 20261105T111951Z-lambda-s3-the-lambda-python-runtime-has-no.md) and its incomplete `os` module (20261105T111951Z-lambda-opensearch-s3-the-lambda-python-runtime-has.md); neither covers layer extraction or sys.path construction, and none of the checks below are marked owned by them.

Cause 1 of 6: layer content is never extracted to /opt, so no module shipped in a layer is importable.
AWS: Smithy models UpdateFunctionConfiguration/CreateFunction member `Layers` (LayersReferenceList of Layer ARNs) and returns them in FunctionConfiguration.Layers. AWS documents that Lambda extracts each attached layer's zip into /opt in the execution environment before init, and that the managed runtimes put /opt/python (and /opt/python/lib/python<X.Y>/site-packages) on the module search path; a module that a layer ships is therefore importable from the handler. The emulator accepts the layer reference and reports it on GetFunctionConfiguration, so the control plane claims the layer is attached.
Emulator: every handler import of a layer-supplied module faults. Invoke answers 200 with X-Amz-Function-Error: Unhandled and a payload whose errorType is Runtime.ImportModuleError (when the import is at module scope) or ModuleNotFoundError (when it is inside the handler), with errorMessage "Unable to import module '<handler module>': No module named '<layer module>'" / "No module named '<layer module>'". The fixed text is AWS's import-error wording; the module names are the ones the request's layer zip and handler supply.
Why: a client that packages shared code or dependencies as a layer gets a function that cannot start. Every layer-dependent behaviour built on top of it (version pinning, detach, delete-after-attach, five-layer limits) is unobservable, so a user validating a layered deployment against the emulator sees failures AWS would not produce.
Fix: the Invoke path's execution-environment setup — layer resolution/extraction into /opt before init, in the Lambda data-plane handler and its runtime sandbox.
Traces: lambda.layer-code-importable, lambda.layer-count-over-five-rejected, lambda.layer-declared-incompatible-runtime-and-architecture-still-attaches-and-loads, lambda.layer-published-version-keeps-its-layer-after-latest-repoints-and-layer-deleted, lambda.layers-deleted-version-still-resolves-new-reference-rejected, lambda.layers-detach-breaks-handler-import, lambda.layers-version-pin-ignores-newer-version, lambda.layers-toplevel-zip-entry-not-importable-rejected, lambda.layer-unchanged-reattach-of-a-deleted-version-still-revalidated.

Cause 2 of 6: the layer's python site-packages directories and .pth files are not added to the module search path.
AWS: recorded behavior only for the exact path list; AWS documents that /opt/python/lib/python<X.Y>/site-packages is a site directory for the matching runtime minor version (and only that one), so .pth files in it extend sys.path and packages in it precede /opt/python.
Emulator: the handler faults with ModuleNotFoundError "No module named '<module>'" for a module placed in the layer's site-packages dir and for a module reachable only through a layer .pth file.
Why: dependency layers built with `pip install -t python/lib/python3.12/site-packages` — the documented layout — do not work, and a minor-version mismatch cannot be distinguished from a correctly built layer.
Fix: runtime sandbox sys.path construction for attached layers (site-dir processing), in the Lambda data-plane runtime setup.
Traces: lambda.layer-python-site-packages-dir-importable-and-precedes-python-root, lambda.layer-python-site-packages-for-other-minor-version-not-importable-rejected, lambda.layer-pth-files-in-layer-site-dirs-extend-sys-path.

Cause 3 of 6: sys.path inside the handler contains neither the task root nor the layer root.
AWS: recorded behavior only; AWS's managed Python runtimes expose /var/task first and the layer roots after it, which is what makes a function-package module shadow a same-named layer module.
Emulator: the handler observes sys.path == ['/usr/local/lib/python314.zip', '/usr/local/lib/python3.14', '/usr/local/lib/python3.14/lib-dynload'] — interpreter stdlib entries only, no /var/task and no /opt. (The concrete directory strings vary per build; the requirement is that the task root and the layer roots are present, in that order.)
Why: shadowing precedence cannot be reproduced, and any handler relying on package-relative imports from the deployment package is at risk.
Fix: runtime sandbox sys.path construction, Lambda data-plane runtime setup.
Traces: lambda.layers-function-package-shadows-layer-module.

Cause 4 of 6: /opt/lib is not on the loader path and the runtime has no _ctypes extension module.
AWS: recorded behavior only; AWS puts /opt/lib on LD_LIBRARY_PATH so a layer-shipped shared object loads by bare name, and the managed Python runtimes ship the full CPython extension set including _ctypes.
Emulator: the invoke answers Unhandled with errorType Runtime.ImportModuleError and errorMessage "Unable to import module '<handler module>': No module named '_ctypes'" — the handler cannot even reach the dlopen.
Why: native-dependency layers (the main reason to use a layer) are unusable, and the failure is reported as the customer's import error rather than as an unsupported feature.
Fix: the runtime sandbox's stdlib/extension-module surface and LD_LIBRARY_PATH composition for layers.
Traces: lambda.layer-lib-dir-on-ld-library-path-loads-shared-object-by-bare-name.

Cause 5 of 6: /opt/bin is not on PATH and the runtime's `os` module is incomplete (no os.stat_result).
AWS: recorded behavior only; AWS puts /opt/bin on PATH so a layer-shipped executable runs by bare name from the handler, and `os.stat_result` exists in every managed Python runtime.
Emulator: the invoke answers Unhandled with errorType Runtime.ImportModuleError and errorMessage "Unable to import module '<handler module>': module 'os' has no attribute 'stat_result'".
Why: a handler that shells out to a layer binary cannot even import, and the diagnosis blames the user's module.
Fix: the runtime sandbox's `os` module surface and PATH composition for layers. (Open issue 20261105T111951Z-lambda-opensearch-s3-the-lambda-python-runtime-has.md reports the same os.stat_result gap under a different service scope; it does not carry this check's replay key.)
Traces: lambda.layers-bin-on-path-executes-layer-binary.

Cause 6 of 6: layer files cannot be reached through filesystem calls at all.
AWS: recorded behavior only; a file a layer ships under /opt is an ordinary readable file, so os.path.getsize('/opt/...') returns its size.
Emulator: the invoke answers Unhandled with errorType OSError and errorMessage "a Lambda function in this emulator has no host filesystem or process to reach".
Why: the layer size/quota behaviour the check probes (combined unzipped size over the 250 MB limit must be refused at UpdateFunctionConfiguration) cannot be reached, and any handler that reads a data file from a layer fails.
Fix: the runtime sandbox's filesystem surface for /opt, plus combined-unzipped-size validation in the UpdateFunctionConfiguration/CreateFunction layer validation.
Traces: lambda.layers-combined-unzipped-size-over-250mb-rejected.

## Evidence
Expected (AWS): a handler with a layer attached imports the layer's module and returns its payload; sys.path includes the task root then the layer roots; /opt/python/lib/python<X.Y>/site-packages is a site directory; /opt/lib is on LD_LIBRARY_PATH; /opt/bin is on PATH; files under /opt are readable.

Actual (emulator), from the replay:
- lambda.layer-code-importable: FunctionError=Unhandled, {"errorMessage":"Unable to import module 'index': No module named '<layer module>'","errorType":"Runtime.ImportModuleError"}
- lambda.layer-count-over-five-rejected / layers-detach-breaks-handler-import / layers-version-pin-ignores-newer-version / layers-toplevel-zip-entry-not-importable-rejected / layers-deleted-version-still-resolves-new-reference-rejected / layer-published-version-keeps-its-layer-after-latest-repoints-and-layer-deleted / layer-declared-incompatible-runtime-and-architecture-still-attaches-and-loads: FunctionError=Unhandled, errorType ModuleNotFoundError, "No module named '<layer module>'"
- lambda.layer-unchanged-reattach-of-a-deleted-version-still-revalidated: AssertionError: positive control: the layer module must import
- lambda.layer-python-site-packages-dir-importable-and-precedes-python-root / layer-python-site-packages-for-other-minor-version-not-importable-rejected / layer-pth-files-in-layer-site-dirs-extend-sys-path: ModuleNotFoundError for the site-packages/.pth-provided module
- lambda.layers-function-package-shadows-layer-module: AssertionError: both the task root and the layer root must be on sys.path; the handler saw ['/usr/local/lib/python314.zip', '/usr/local/lib/python3.14', '/usr/local/lib/python3.14/lib-dynload']
- lambda.layer-lib-dir-on-ld-library-path-loads-shared-object-by-bare-name: Runtime.ImportModuleError "Unable to import module 'index': No module named '_ctypes'"
- lambda.layers-bin-on-path-executes-layer-binary: Runtime.ImportModuleError "Unable to import module 'index': module 'os' has no attribute 'stat_result'"
- lambda.layers-combined-unzipped-size-over-250mb-rejected: OSError "a Lambda function in this emulator has no host filesystem or process to reach" from os.path.getsize('/opt/blob/zeros.bin')

## Steps to Reproduce
1. PublishLayerVersion with a zip containing python/<module>.py (and variants: python/lib/python<X.Y>/site-packages/<pkg>, a .pth file in that site dir, lib/<libfoo>.so, bin/<tool>, blob/<datafile>).
2. CreateFunction (python3.x, Zip) with Layers=[<that layer version ARN>] and a handler that imports the layer module / inspects sys.path / dlopens the shared object by bare name / execs the layer binary / os.path.getsize the data file.
3. Invoke the function.
4. Observe 200 with X-Amz-Function-Error: Unhandled and a Runtime.ImportModuleError / ModuleNotFoundError / OSError payload instead of the handler's result.
5. Re-replay the check ids in trace_ids to confirm.
