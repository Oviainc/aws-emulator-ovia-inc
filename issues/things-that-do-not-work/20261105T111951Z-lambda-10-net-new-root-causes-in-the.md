Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.50-distinct-code-artifacts-no-shared-code-store lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start lambda.exec-wrapper-naming-a-missing-file-rejected-at-invoke lambda.exec-wrapper-script-starts-the-runtime-with-its-environment-and-arguments lambda.extension-register-after-init-is-refused lambda.extension-that-never-registers-fails-the-invoke lambda.external-extension-layer-runs-identically-across-runtime-families lambda.external-extension-that-exits-before-registering-fails-the-invoke lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected lambda.init-phase-is-not-charged-against-the-function-timeout lambda.invoke-during-a-memory-update-is-served-and-converges-to-the-new-size lambda.log-lambda-report-init-duration-only-on-cold-report-line lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier lambda.log-lambda-report-system-log-level-filters-platform-records-independently lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream lambda.log-tail-absent-unless-log-type-is-tail lambda.log-tail-application-log-level-update-admits-debug-on-next-invoke lambda.log-tail-init-phase-output-precedes-first-start-only lambda.log-tail-not-returned-for-dry-run lambda.log-tail-not-returned-for-event-invocation lambda.log-tail-python-json-extra-fields-merge-into-the-record lambda.log-tail-python-json-logging-exception-carries-error-fields lambda.log-tail-python-json-unhandled-error-record-uses-log-level-key lambda.log-tail-python-text-format-defaults-root-logger-to-warning lambda.log-tail-runtime-env-exposes-log-format-and-level lambda.log-tail-utf8-output-round-trips-byte-exact lambda.memory-size-seen-by-the-handler-tracks-a-configuration-update lambda.out-of-memory-kill-discards-the-execution-environment lambda.oversize-unhandled-error-becomes-response-size-too-large-rejected lambda.published-version-keeps-its-own-memory-limit-rejected-past-it lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it lambda.python-builtin-module-as-handler-rejected lambda.python-client-context-reaches-handler-as-typed-object lambda.python-context-request-id-equals-invoke-response-request-id lambda.python-context-version-and-invoked-arn-follow-the-qualifier lambda.python-custom-exception-class-names-error-type-and-frames lambda.python-dotted-handler-import-failures-name-the-dotted-module lambda.python-event-payload-forms-reach-the-handler-as-parsed-json lambda.python-exception-error-message-is-str-of-the-exception lambda.python-handler-in-nested-package-resolves-relative-imports-and-files lambda.python-invalid-client-context-rejected-before-the-handler-runs lambda.python-malformed-handler-name-rejected-at-invoke lambda.python-module-level-exception-rejected-with-its-own-error-type lambda.python-module-state-is-per-version-not-shared-with-latest lambda.python-non-serializable-return-rejected-with-marshal-error lambda.python-remaining-time-is-bounded-by-the-configured-timeout lambda.python-return-bytes-and-non-finite-floats-pass-through-unvalidated lambda.python-return-values-serialize-to-exact-json-bytes lambda.python-runtime-defined-env-vars-match-the-runtime-identity lambda.python-syntax-error-in-handler-module-rejected-at-invoke lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state lambda.python-trailing-data-after-json-fails-runtime-unmarshal lambda.python-unparseable-payload-rejected-before-the-handler-runs lambda.python-user-env-vars-and-tz-override-reach-the-process lambda.remaining-time-starts-at-the-configured-timeout-and-counts-down lambda.snapstart-python-version-restores-with-before-snapshot-state-and-a-restore-report lambda.sync-request-at-the-6mb-limit-reaches-the-handler-intact-one-byte-over-rejected lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it lambda.function-sdk-reaches-the-emulator lambda.handler-import-failure-raises-rather-than-echoing lambda.recursion-config-terminate-stops-a-self-invocation-chain lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after lambda.ephemeral-storage-available lambda.ephemeral-storage-size-is-enforced-not-only-reported lambda.arm64-function-reports-aarch64-at-runtime lambda.deep-lambda-runtime-truth-ephemeral-storage-update-resizes-tmp-in-next-environment lambda.deep-lambda-runtime-truth-tmp-empty-in-fresh-env-and-survives-warm lambda.log-lambda-report-stdout-stderr-and-logging-lines-framed-in-stream lambda.package-symlinks-extracted-as-symlinks-and-followed lambda.python-runtime-family-every-supported-minor-runs-identically lambda.task-root-and-runtime-dir-read-only-while-tmp-writable-rejected lambda.tmp-writable lambda.tmp-persists-across-warm lambda.tmp-not-shared-between-two-distinct-functions lambda.deep-lambda-runtime-truth-layer-files-byte-exact-at-opt-and-opt-write-rejected lambda.package-file-modes-normalized-so-every-file-is-readable-and-executable lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected lambda.layer-code-importable lambda.layer-count-over-five-rejected lambda.layer-declared-incompatible-runtime-and-architecture-still-attaches-and-loads lambda.layer-lib-dir-on-ld-library-path-loads-shared-object-by-bare-name lambda.layer-module-overrides-runtime-bundled-boto3 lambda.layer-pth-files-in-layer-site-dirs-extend-sys-path lambda.layer-published-version-keeps-its-layer-after-latest-repoints-and-layer-deleted lambda.layer-python-site-packages-dir-importable-and-precedes-python-root lambda.layer-python-site-packages-for-other-minor-version-not-importable-rejected lambda.layer-unchanged-reattach-of-a-deleted-version-still-revalidated lambda.layers-bin-on-path-executes-layer-binary lambda.layers-combined-unzipped-size-over-250mb-rejected lambda.layers-deleted-version-still-resolves-new-reference-rejected lambda.layers-detach-breaks-handler-import lambda.layers-function-package-shadows-layer-module lambda.layers-toplevel-zip-entry-not-importable-rejected lambda.layers-version-pin-ignores-newer-version lambda.two-layers-sharing-a-path-resolve-to-the-last-layer-at-runtime lambda.external-extension-registers-receives-invoke-and-shutdown-events lambda.function-url-missing-permission-not-invoked lambda.stdlib-imports lambda.runtime-update-reflected-in-sys-version lambda.snapstart-restored-version-reuses-the-snapshot-init lambda.concurrency-throttles-at-limit lambda.environment-identity-warm-reuse-and-config-update-forces-cold lambda.stream-provisioned-environment-reports-provisioned-initialization-type lambda.reserved-concurrency-is-one-pool-shared-by-latest-version-and-alias-rejected lambda.reserved-concurrency-zero-on-one-function-leaves-another-unthrottled-rejected lambda.reserved-zero-blocks-invoke lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot lambda.timed-out-invocation-releases-its-reserved-concurrency-slot lambda.provisioned-concurrency-invocation-reports-no-init-duration lambda.alias-weighted-routing-splits-traffic lambda.alias-weight-ladder-observed-distribution
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.50-distinct-code-artifacts-no-shared-code-store|_invoke#29|KeyError|'request_id': resp['ResponseMetadata']['RequestId']}
Issue-Key: lambda.allocation-beyond-memory-size-is-killed-out-of-memory-rejected|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected|_invoke#8|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start|_invoke#8|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.exec-wrapper-naming-a-missing-file-rejected-at-invoke|_invoke#4|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"]}
Issue-Key: lambda.exec-wrapper-script-starts-the-runtime-with-its-environment-and-arguments|_invoke#5|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"]}
Issue-Key: lambda.extension-register-after-init-is-refused|_invoke#7|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"]}
Issue-Key: lambda.extension-that-never-registers-fails-the-invoke|_invoke#7|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"]}
Issue-Key: lambda.external-extension-layer-runs-identically-across-runtime-families|_invoke#7|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.external-extension-that-exits-before-registering-fails-the-invoke|_invoke#7|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"]}
Issue-Key: lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.init-phase-is-not-charged-against-the-function-timeout|_invoke#6|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.invoke-during-a-memory-update-is-served-and-converges-to-the-new-size|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-init-duration-only-on-cold-report-line|_invoke#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field|_invoke#8|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log|_invoke#7|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one|_invoke#7|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id|_invoke#7|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier|_invoke#12|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-system-log-level-filters-platform-records-independently|_invoke#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail|_invoke#7|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream|_invoke#12|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-absent-unless-log-type-is-tail|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-application-log-level-update-admits-debug-on-next-invoke|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-init-phase-output-precedes-first-start-only|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-not-returned-for-dry-run|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-not-returned-for-event-invocation|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-python-json-extra-fields-merge-into-the-record|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-python-json-logging-exception-carries-error-fields|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-python-json-unhandled-error-record-uses-log-level-key|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-python-text-format-defaults-root-logger-to-warning|_invoke_tail#8|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-runtime-env-exposes-log-format-and-level|_invoke_tail#5|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.log-tail-utf8-output-round-trips-byte-exact|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.memory-size-seen-by-the-handler-tracks-a-configuration-update|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.out-of-memory-kill-discards-the-execution-environment|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.oversize-unhandled-error-becomes-response-size-too-large-rejected|_invoke#6|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.published-version-keeps-its-own-memory-limit-rejected-past-it|_invoke#8|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it|_invoke#8|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.python-builtin-module-as-handler-rejected|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-client-context-reaches-handler-as-typed-object|_invoke#5|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-context-request-id-equals-invoke-response-request-id|_invoke#4|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-context-version-and-invoked-arn-follow-the-qualifier|_invoke#9|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-custom-exception-class-names-error-type-and-frames|_invoke#4|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-dotted-handler-import-failures-name-the-dotted-module|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-event-payload-forms-reach-the-handler-as-parsed-json|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-exception-error-message-is-str-of-the-exception|_invoke#6|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-handler-in-nested-package-resolves-relative-imports-and-files|_invoke#6|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-invalid-client-context-rejected-before-the-handler-runs|_invoke#5|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-malformed-handler-name-rejected-at-invoke|_invoke#5|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-module-level-exception-rejected-with-its-own-error-type|_invoke#5|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-module-state-is-per-version-not-shared-with-latest|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-non-serializable-return-rejected-with-marshal-error|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-remaining-time-is-bounded-by-the-configured-timeout|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-return-bytes-and-non-finite-floats-pass-through-unvalidated|_invoke#4|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-return-values-serialize-to-exact-json-bytes|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-runtime-defined-env-vars-match-the-runtime-identity|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-syntax-error-in-handler-module-rejected-at-invoke|_invoke#4|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-trailing-data-after-json-fails-runtime-unmarshal|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-unparseable-payload-rejected-before-the-handler-runs|_invoke#3|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.python-user-env-vars-and-tz-override-reach-the-process|_invoke#5|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"],
Issue-Key: lambda.remaining-time-starts-at-the-configured-timeout-and-counts-down|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.snapstart-python-version-restores-with-before-snapshot-state-and-a-restore-report|_invoke#10|KeyError|"request_id": resp["ResponseMetadata"]["RequestId"]}
Issue-Key: lambda.sync-request-at-the-6mb-limit-reaches-the-handler-intact-one-byte-over-rejected|_invoke#7|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it|_invoke#5|KeyError|return resp, body, tail, resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.function-sdk-reaches-the-emulator|_invoke#18|AssertionError|raise AssertionError(
Issue-Key: lambda.handler-import-failure-raises-rather-than-echoing|run#15|AssertionError|assert sdk_error is None, (
Issue-Key: lambda.recursion-config-terminate-stops-a-self-invocation-chain|_run_chain#10|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after|run#10|AssertionError|assert not ctl.get("FunctionError") and ctl_body == {"label": "control", "tagged": True}, (
Issue-Key: lambda.ephemeral-storage-available|_invoke_sync#5|AssertionError|raise AssertionError(
Issue-Key: lambda.ephemeral-storage-size-is-enforced-not-only-reported|_invoke_json#7|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.arm64-function-reports-aarch64-at-runtime|_body#8|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.deep-lambda-runtime-truth-ephemeral-storage-update-resizes-tmp-in-next-environment|_fill#5|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.deep-lambda-runtime-truth-tmp-empty-in-fresh-env-and-survives-warm|_invoke#5|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.log-lambda-report-stdout-stderr-and-logging-lines-framed-in-stream|_invoke#7|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.package-symlinks-extracted-as-symlinks-and-followed|_invoke_ok#10|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.python-runtime-family-every-supported-minor-runs-identically|_invoke_sync#5|AssertionError|raise AssertionError(
Issue-Key: lambda.task-root-and-runtime-dir-read-only-while-tmp-writable-rejected|_invoke_ok#9|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.tmp-writable|_invoke_sync#4|AssertionError|raise AssertionError(
Issue-Key: lambda.tmp-persists-across-warm|_invoke_sync#5|AssertionError|raise AssertionError(
Issue-Key: lambda.tmp-not-shared-between-two-distinct-functions|_invoke_sync#9|AssertionError|raise AssertionError(
Issue-Key: lambda.deep-lambda-runtime-truth-layer-files-byte-exact-at-opt-and-opt-write-rejected|_invoke#10|AssertionError|assert not resp.get("FunctionError"), (
Issue-Key: lambda.package-file-modes-normalized-so-every-file-is-readable-and-executable|_invoke_ok#8|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected|_invoke#6|AssertionError|assert "FunctionError" not in resp, f"handler failed with {resp.get('FunctionError')!r}: {raw!r}"
Issue-Key: lambda.layer-code-importable|_invoke_sync#12|AssertionError|raise AssertionError(
Issue-Key: lambda.layer-count-over-five-rejected|run#11|AssertionError|assert "FunctionError" not in resp, resp["Payload"].read()
Issue-Key: lambda.layer-declared-incompatible-runtime-and-architecture-still-attaches-and-loads|_invoke_ok#13|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-lib-dir-on-ld-library-path-loads-shared-object-by-bare-name|_invoke_ok#10|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-module-overrides-runtime-bundled-boto3|_invoke_ok#8|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-pth-files-in-layer-site-dirs-extend-sys-path|_invoke_ok#9|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-published-version-keeps-its-layer-after-latest-repoints-and-layer-deleted|_invoke_ok#20|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-python-site-packages-dir-importable-and-precedes-python-root|_invoke_ok#9|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-python-site-packages-for-other-minor-version-not-importable-rejected|_invoke_ok#8|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layer-unchanged-reattach-of-a-deleted-version-still-revalidated|run#7|AssertionError|assert _invoke(lam, fn) == ({"marker": token}, None), "positive control: the layer module must import"
Issue-Key: lambda.layers-bin-on-path-executes-layer-binary|_invoke_json#11|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-combined-unzipped-size-over-250mb-rejected|_invoke_ok#10|AssertionError|assert error is None, f"{fn} raised ({error}) instead of returning: {str(body)[:600]}"
Issue-Key: lambda.layers-deleted-version-still-resolves-new-reference-rejected|_invoke_json#12|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-detach-breaks-handler-import|_invoke_json#11|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-function-package-shadows-layer-module|run#19|AssertionError|assert "/var/task" in syspath and "/opt/python" in syspath, (
Issue-Key: lambda.layers-toplevel-zip-entry-not-importable-rejected|_invoke_json#13|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.layers-version-pin-ignores-newer-version|_invoke_json#16|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.two-layers-sharing-a-path-resolve-to-the-last-layer-at-runtime|_invoke_json#12|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.external-extension-registers-receives-invoke-and-shutdown-events|run#20|AssertionError|assert last["ext_log_exists"], (
Issue-Key: lambda.function-url-missing-permission-not-invoked|run#22|AssertionError|assert granted is not None, (
Issue-Key: lambda.stdlib-imports|run#7|AssertionError|assert resp["python_version"] == expected_version, (
Issue-Key: lambda.runtime-update-reflected-in-sys-version|run#5|AssertionError|assert _invoke(lam, fn) == ("$LATEST", {"python": "3.12"}), "python3.12 must run 3.12"
Issue-Key: lambda.snapstart-restored-version-reuses-the-snapshot-init|run#16|AssertionError|assert all(b["hook_import"] == "ok" for b in ver_bodies), (
Issue-Key: lambda.concurrency-throttles-at-limit|run#19|AssertionError|raise AssertionError(
Issue-Key: lambda.environment-identity-warm-reuse-and-config-update-forces-cold|run#11|AssertionError|assert len(warm_envs) < len(warm), (
Issue-Key: lambda.stream-provisioned-environment-reports-provisioned-initialization-type|run#21|AssertionError|assert warm["init_type"] == "provisioned-concurrency", (
Issue-Key: lambda.reserved-concurrency-is-one-pool-shared-by-latest-version-and-alias-rejected|_probe_while_held#10|Timeout|outcome, detail = _try_invoke(invoker, fn, {}, probe_qualifier)
Issue-Key: lambda.reserved-concurrency-zero-on-one-function-leaves-another-unthrottled-rejected|run#11|Timeout|assert time.monotonic() < deadline, "ReservedConcurrentExecutions=0 never throttled for 60 s"
Issue-Key: lambda.reserved-zero-blocks-invoke|_poll_marker#8|Timeout|time.sleep(1.0)
Issue-Key: lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot|_probe_while_held#7|Timeout|outcome, detail = _try_invoke(invoker, fn, {}, probe_qualifier)
Issue-Key: lambda.timed-out-invocation-releases-its-reserved-concurrency-slot|_probe_while_held#7|Timeout|outcome, detail = _try_invoke(invoker, fn, {}, probe_qualifier)
Issue-Key: lambda.provisioned-concurrency-invocation-reports-no-init-duration|_invoke_tail#6|AssertionError|assert "FunctionError" not in resp, (
Issue-Key: lambda.alias-weighted-routing-splits-traffic|run#20|AssertionError|assert set(seen) == {v1, v2}, (
Issue-Key: lambda.alias-weight-ladder-observed-distribution|_sample#16|Timeout|return {"rows": rows, "counts": counts, "bad": [r for r in rows if r[2] is not None]}
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# lambda: 10 net-new root causes in the Invoke data plane and the Python execution environment

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Ten distinct net-new root causes, all in the Lambda Invoke data plane / execution environment. None is covered by an open issue; the existing open issues 20261105T080515Z-...-nodejs22-x..., 20261029T211043Z-lambda-data-plane-function-url-endpoints-are-unrouted.md and 20261029T211043Z-lambda-control-plane-accepts-eight-classes-of-request.md own other checks, not these.

Cause 1 of 10 — Invoke responses carry no request id.
AWS: Smithy: Invoke is POST /2015-03-31/functions/{FunctionName}/invocations, restJson1; every AWS response carries the `x-amzn-RequestId` response header, which botocore surfaces as `ResponseMetadata.RequestId` and which is the same id the runtime reports as `context.aws_request_id` and as the id in the START/END/REPORT log lines.
Emulator: the Invoke response headers are only content-type, x-amz-function-error, x-amz-executed-version, content-length and date — no `x-amzn-RequestId` — so `resp["ResponseMetadata"]["RequestId"]` raises KeyError. One check asserts it directly: "Invoke response carries no request id (x-amzn-RequestId)".
Why: a client cannot correlate an invocation with its CloudWatch log stream, its REPORT line or a support case; every retry/idempotency and tracing path keyed on the invoke request id breaks.
Fix: the Invoke/InvokeWithResponseStream handler's response serializer in the lambda crate must emit the request id header on every response, success and error alike.
Traces: lambda.python-context-request-id-equals-invoke-response-request-id, lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id and the ~60 further ids in trace_ids.

Cause 2 of 10 — the managed python runtime ships no AWS SDK.
AWS: the python3.x managed runtimes include boto3/botocore on sys.path; `import boto3` in a handler succeeds.
Emulator: `Runtime.ImportModuleError` / errorMessage "Unable to import module 'index': No module named 'boto3'" with FunctionError=Unhandled and HTTP 200.
Why: no handler can call any AWS service, so every in-function integration and every IAM-boundary check is unobservable.
Fix: the python runtime's module search path / bundled site-packages in the lambda runtime component.
Traces: lambda.function-sdk-reaches-the-emulator, lambda.handler-import-failure-raises-rather-than-echoing, lambda.recursion-config-terminate-stops-a-self-invocation-chain, lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after.

Cause 3 of 10 — the execution environment has no filesystem or process view.
AWS: a handler may call os.statvfs('/tmp'), os.write(1, ...), os.readlink, platform.machine()/os.uname(), and open('/tmp/<f>','w'); /tmp is writable up to EphemeralStorage.Size, /var/task and /opt are read-only.
Emulator: those calls raise `OSError` with errorMessage "a Lambda function in this emulator has no host filesystem or process to reach" or "a Lambda function in this emulator cannot open a file of the host"; one import even fails with Runtime.ImportModuleError carrying that same text.
Why: /tmp, ephemeral storage sizing, architecture reporting and read-only-root semantics are all unobservable, and ordinary handler code that touches the filesystem faults.
Fix: the python runtime sandbox's filesystem/process layer in the lambda runtime component.
Traces: lambda.ephemeral-storage-available, lambda.ephemeral-storage-size-is-enforced-not-only-reported, lambda.arm64-function-reports-aarch64-at-runtime, lambda.deep-lambda-runtime-truth-ephemeral-storage-update-resizes-tmp-in-next-environment, lambda.deep-lambda-runtime-truth-tmp-empty-in-fresh-env-and-survives-warm, lambda.log-lambda-report-stdout-stderr-and-logging-lines-framed-in-stream, lambda.package-symlinks-extracted-as-symlinks-and-followed, lambda.python-runtime-family-every-supported-minor-runs-identically, lambda.task-root-and-runtime-dir-read-only-while-tmp-writable-rejected, lambda.tmp-writable, lambda.tmp-persists-across-warm, lambda.tmp-not-shared-between-two-distinct-functions.

Cause 4 of 10 — CPython extension modules are missing from the runtime.
AWS: the python managed runtime is a complete CPython: `_signal`, `_socket` and the modules that import them (signal, socket, ssl, http.client) are importable.
Emulator: Runtime.ImportModuleError "No module named '_signal'" and "No module named '_socket'".
Why: any handler importing socket/ssl/signal — i.e. almost any networking or timeout-handling code — dies at init.
Fix: the embedded CPython build/module table in the lambda runtime component.
Traces: lambda.deep-lambda-runtime-truth-layer-files-byte-exact-at-opt-and-opt-write-rejected, lambda.package-file-modes-normalized-so-every-file-is-readable-and-executable, lambda.outbound-dns-resolution-failure-for-unresolvable-hostname-rejected.

Cause 5 of 10 — layer content is never mounted into the execution environment.
AWS: each attached layer version is extracted under /opt; for python, /opt/python and /opt/python/lib/python<M.m>/site-packages are on sys.path, /opt/bin is on PATH and /opt/lib on LD_LIBRARY_PATH, later layers overwrite earlier ones at a shared path, and detaching a layer makes its modules unimportable again.
Emulator: a handler importing a module published in an attached layer fails — ModuleNotFoundError/Runtime.ImportModuleError "No module named '<layer module>'" — so nothing from /opt is visible.
Why: every layer-based deployment (shared libraries, native .so, CLI binaries, .pth site hooks, version pinning) is non-functional, and the negative controls prove nothing.
Fix: the layer-extraction/mount step of the invoke path, and the sys.path/PATH/LD_LIBRARY_PATH composition in the python runtime.
Traces: lambda.layer-code-importable, lambda.layer-count-over-five-rejected, lambda.layer-declared-incompatible-runtime-and-architecture-still-attaches-and-loads, lambda.layer-lib-dir-on-ld-library-path-loads-shared-object-by-bare-name, lambda.layer-module-overrides-runtime-bundled-boto3, lambda.layer-pth-files-in-layer-site-dirs-extend-sys-path, lambda.layer-published-version-keeps-its-layer-after-latest-repoints-and-layer-deleted, lambda.layer-python-site-packages-dir-importable-and-precedes-python-root, lambda.layer-python-site-packages-for-other-minor-version-not-importable-rejected, lambda.layer-unchanged-reattach-of-a-deleted-version-still-revalidated, lambda.layers-bin-on-path-executes-layer-binary, lambda.layers-combined-unzipped-size-over-250mb-rejected, lambda.layers-deleted-version-still-resolves-new-reference-rejected, lambda.layers-detach-breaks-handler-import, lambda.layers-function-package-shadows-layer-module, lambda.layers-toplevel-zip-entry-not-importable-rejected, lambda.layers-version-pin-ignores-newer-version, lambda.two-layers-sharing-a-path-resolve-to-the-last-layer-at-runtime.

Cause 6 of 10 — external extensions under /opt/extensions are never started.
AWS: every executable under /opt/extensions is launched before init, registers against the Extensions API, receives INVOKE and SHUTDOWN events, and an extension that exits before registering fails the invocation.
Emulator: nothing under /opt/extensions/ is executed — the extension never produces its side effect.
Why: observability/agent layers (the normal way telemetry vendors attach) do not run, and the refusal paths that depend on registration never fire.
Fix: the execution-environment bootstrap in the lambda runtime component (extension discovery + Extensions API).
Traces: lambda.external-extension-registers-receives-invoke-and-shutdown-events.

Cause 7 of 10 — function-URL endpoints are not routed; the URL host answers a service-dispatch 404.
AWS: a GET on a function URL is served by the handler (200 with the mapped result), an unauthorised anonymous GET is 403, and an OPTIONS preflight on a CORS-enabled URL is answered 200 without invoking the handler.
Emulator: HTTP 404 with body {"__type":"UnknownService","message":"no emulated service is registered for '<url id>'"} (and the same for 'lambda'), i.e. the request never reaches a function-URL route at all.
Why: every function-URL client gets a 404 with an AWS-internal dispatch error instead of the handler's response, and the auth/CORS rules are unobservable.
Fix: the HTTP front-end routing for function-URL hosts, ahead of the per-service dispatcher.
Traces: lambda.function-url-missing-permission-not-invoked, lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler (and the function-URL 404 also surfaces in lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected, lambda.deep-lambda-3-url-cors-not-enforced-server-side).

Cause 8 of 10 — the runtime identifier does not select the interpreter version, and SnapStart's python hook module is absent.
AWS: Runtime=python3.12 runs CPython 3.12, so sys.version_info[:2] == (3, 12); the python runtime provides the `snapshot_restore_py` module so before/after-snapshot hooks can register.
Emulator: the handler reports sys.version_info[:2] == [3, 14] under Runtime=python3.12 ("python3.12 must run 3.12"), and `import snapshot_restore_py` raises ModuleNotFoundError so no restore hook ever registers.
Why: version-sensitive code and wheels resolve against the wrong interpreter, and SnapStart restore semantics cannot be exercised.
Fix: the runtime-to-interpreter mapping and bundled runtime support modules in the lambda runtime component.
Traces: lambda.stdlib-imports, lambda.runtime-update-reflected-in-sys-version, lambda.snapstart-restored-version-reuses-the-snapshot-init.

Cause 9 of 10 — concurrency is not enforced and execution environments are never reused.
AWS: with ReservedConcurrentExecutions=1 a second concurrent invoke is throttled (429 TooManyRequestsException / Throttles); serialized invocations reuse the same warm environment (same module-level state and env id); an invocation served by a READY provisioned environment sees AWS_LAMBDA_INITIALIZATION_TYPE=provisioned-concurrency.
Emulator: no invoke beside a holder is throttled at ReservedConcurrentExecutions=1; six serialized invocations each report a fresh env id, so no environment is ever reused; a provisioned-concurrency invocation reports AWS_LAMBDA_INITIALIZATION_TYPE='on-demand'.
Why: throttle-driven back-pressure, cold/warm-start accounting and provisioned concurrency are all misreported, so capacity planning and caching assumptions silently fail.
Fix: the concurrency admission control and the environment pool in the invoke path.
Traces: lambda.concurrency-throttles-at-limit, lambda.environment-identity-warm-reuse-and-config-update-forces-cold, lambda.stream-provisioned-environment-reports-provisioned-initialization-type, lambda.reserved-concurrency-is-one-pool-shared-by-latest-version-and-alias-rejected, lambda.reserved-concurrency-zero-on-one-function-leaves-another-unthrottled-rejected, lambda.reserved-zero-blocks-invoke, lambda.out-of-memory-invocation-releases-its-reserved-concurrency-slot, lambda.timed-out-invocation-releases-its-reserved-concurrency-slot, lambda.provisioned-concurrency-invocation-reports-no-init-duration.

Cause 10 of 10 — weighted alias routing ignores AdditionalVersionWeights.
AWS: an alias with RoutingConfig.AdditionalVersionWeights splitting 50/50 sends invocations to both versions; ExecutedVersion varies across invokes.
Emulator: 40 invokes through a 50/50 alias all report FunctionVersion=1 — the weights are not consulted.
Why: canary/linear deployments route 100% of traffic to one version while the control plane reports a split, which is exactly the failure mode a canary is meant to prevent.
Fix: the alias resolution step of the invoke path (alias state → version selection).
Traces: lambda.alias-weighted-routing-splits-traffic, lambda.alias-weight-ladder-observed-distribution.

## Evidence
Cause 1: `resp["ResponseMetadata"]["RequestId"]` -> KeyError: 'RequestId'; one check asserts "Invoke response carries no request id (x-amzn-RequestId)". Observed Invoke response headers: content-type, x-amz-function-error, x-amz-executed-version, content-length, date. Expected: the standard `x-amzn-RequestId` header on every response.
Cause 2: HTTP 200, FunctionError=Unhandled, payload {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}. Expected: the handler's own result.
Cause 3: payload {"errorType":"OSError","errorMessage":"a Lambda function in this emulator has no host filesystem or process to reach"} from os.statvfs('/tmp'), os.write(1,...), os.readlink, platform.machine(); and "a Lambda function in this emulator cannot open a file of the host" from open('/tmp/...','w'). Expected: success.
Cause 4: {"errorType":"Runtime.ImportModuleError","errorMessage":"Unable to import module 'index': No module named '_signal'"} and the same for '_socket'. Expected: importable.
Cause 5: {"errorType":"Runtime.ImportModuleError","errorMessage":"Unable to import module 'index': No module named '<layer module>'"} / ModuleNotFoundError for a module shipped in an attached layer. Expected: importable from /opt/python.
Cause 6: the extension produced no side effect — "nothing under /opt/extensions/ was executed".
Cause 7: GET/OPTIONS on the function URL -> HTTP 404, x-amzn-errortype: UnknownService, body {"__type":"UnknownService","message":"no emulated service is registered for 'lambda'"}. Expected: 200 from the handler, 403 for an unauthorised anonymous GET, 200 for the preflight.
Cause 8: handler reports sys.version_info[:2]==[3,14] for Runtime=python3.12; import snapshot_restore_py -> ModuleNotFoundError.
Cause 9: at ReservedConcurrentExecutions=1 a concurrent invoke returned ('ok', 200) instead of being throttled; six serialized invokes reported six distinct env ids; provisioned invocation reported init_type 'on-demand' instead of 'provisioned-concurrency'.
Cause 10: 40 invokes through a 50/50 weighted alias observed {'1': 40}.

## Steps to Reproduce
1. CreateFunction (Runtime=python3.12, Handler=index.handler) and Invoke it; read resp['ResponseMetadata']['RequestId'] -> KeyError (cause 1).
2. Make the handler `import boto3` -> Runtime.ImportModuleError (cause 2).
3. Make the handler call os.statvfs('/tmp') / platform.machine() / open('/tmp/x','w') -> OSError (cause 3).
4. `import socket` or `import signal` -> Runtime.ImportModuleError for _socket/_signal (cause 4).
5. PublishLayerVersion with a python module, attach it, import it from the handler -> ModuleNotFoundError (cause 5).
6. Publish a layer with an executable under extensions/ and invoke -> no extension side effect (cause 6).
7. CreateFunctionUrlConfig and GET the URL -> 404 UnknownService (cause 7).
8. Have the handler report sys.version_info and import snapshot_restore_py (cause 8).
9. PutFunctionConcurrency ReservedConcurrentExecutions=1 and invoke concurrently; invoke serially and compare env ids; PutProvisionedConcurrencyConfig and read AWS_LAMBDA_INITIALIZATION_TYPE (cause 9).
10. CreateAlias with RoutingConfig 50/50 over two versions and invoke 40 times; inspect ExecutedVersion (cause 10).
