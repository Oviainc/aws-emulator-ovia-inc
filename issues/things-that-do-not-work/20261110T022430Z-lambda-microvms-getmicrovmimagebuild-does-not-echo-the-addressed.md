Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-customer-image-lifecycle
Issue-Step: task#9
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-customer-image-lifecycle
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|GetMicrovmImageBuild|2xx->2xx|$.buildId
Issue-Commit: 505d23cb73aeec2b6bc1a547ffda8ada5cc353f2

# lambda-microvms GetMicrovmImageBuild does not echo the addressed build identifier verbatim

> _Note: this issue was identified against the code base at commit `505d23cb73aeec2b6bc1a547ffda8ada5cc353f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImageBuild

## Description
This issue carries 1 root cause in the lambda-microvms emulator, observed on replay of wire corpus simparity-wire-aws-v1. It is the ONLY cluster of this replay that the report shows with an EMPTY stub_path and an EMPTY owned_by; every other cluster of the run is already owned by an open issue.

=== Cause 1 of 1: the single-build read returns a buildId that is not identical to the identifier recorded for the build the request addressed ===
Fixture lambdamicrovms-customer-image-lifecycle, step task#9 (GetMicrovmImageBuild, HTTP 200 on both sides).

AWS: Smithy GetMicrovmImageBuild is GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds/{buildId}, 200; buildId is an @httpLabel on the INPUT and a @required member of GetMicrovmImageBuildOutput targeting NonBlankString (@length 1..2048, pattern ^[^\s]+$), documented 'The build request ID.' Because the build is addressed BY that identifier in the URI path, the identifier AWS reports in the 200 body is the very identifier the request named, character for character — the read is an echo of the addressed resource key, and the same key is what ListMicrovmImageBuilds reports for that row (MicrovmImageBuildSummary$buildId, same target shape). The pattern ^[^\s]+$ additionally forbids any whitespace anywhere in the value, so a value that carries leading, trailing or embedded whitespace is not a value this member may hold at all. Nothing in the model or the recording makes the identifier's TEXT a requirement (the corpus asserts it only as a shaped, non-asserted value on the listing rows of the same fixture) — what is a requirement is that the value the read answers with is the SAME value as the one the request addressed and the one the listing reported.

Emulator: the replay reports $.buildId: value_mismatch at this step, expected 'build-0000000000000000', got 'build-0000000000000000' — HTTP 200 on both sides and every other member of the body agreeing. The two values render as the same 'build-<digits>' text, so the inequality is in the exact characters of the value rather than in its visible form: a padding/width difference, surrounding or embedded whitespace (which the member's own pattern ^[^\s]+$ forbids), or a non-ASCII digit in the minted identifier. The read itself is a straight projection of the stored record (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image_build.rs:220-239 looks the build up by `version.builds.get(&input.build_id)` and answers `build_id: build.build_id.clone()`), and the listing projects the same field (src/ops/list_microvm_image_builds.rs:269), so the value that diverges is the identifier the service MINTED and stored for the build, not a per-path projection difference — the stored id is not the identifier the corpus records for that build, and it is served unchanged to every reader of that build.

Why: buildId is the only handle a client has on a build. A client polls ListMicrovmImageBuilds, keeps the row's buildId, and then reads or compares that build by it — and the listing cursor of this very service is built on the id as well (src/ops/list_microvm_image_builds.rs:245-258 pages by `build.build_id` ordering and encodes it into nextToken). An identifier that is not byte-identical to the one it is addressed by breaks any client that round-trips it: an equality check against the id it just requested fails, a map keyed on the id acquires two entries for one build, and a build id carrying whitespace is not even a legal value for the member, so an SDK with constraint validation enabled rejects the response or re-sending the id in a URI path produces a different, unresolvable path.

Fix: the build record's identifier in the service state — the id minted when a build is recorded for a version, which get_microvm_image_build.rs and list_microvm_image_builds.rs both clone out of `crate::state::MicrovmImageBuild::build_id` — must be minted in the form the recording shows for a build id and must be stored and echoed with no padding, whitespace or non-ASCII character added, so that the single-build read answers exactly the identifier the request addressed and the identifier the listing reported. This belongs in the state's build-id allocation (and, if the minted id is reshaped on the way out, in the single-build read's projection), not in the authorization/lookup path above it, which the replay shows agreeing.

Related open issue, which does NOT carry this replay key: '20261108T125129Z-lambda-microvms-image-version-and-build-read-paths.md' touches GetMicrovmImageBuild only in passing (its Cause 2 asks that the single-build read gain the recorded member terminationReasonCode, keyed on ListMicrovmImageBuilds#task#8); it holds no key for GetMicrovmImageBuild and never exercises the step below, and the replay reports this cluster with no owning issue.

Traces: lambdamicrovms-customer-image-lifecycle#task#9.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200 and the data listener accepting; 104 steps, 78 passed, 26 mismatched, score 75.0%, 19 divergence clusters, bootstrap_signal false). Expected = recorded AWS response, Actual = emulator.

Cause 1 — lambdamicrovms-customer-image-lifecycle#task#9, GetMicrovmImageBuild, status 200 vs 200:
  $.buildId: value_mismatch (expected 'build-0000000000000000', got 'build-0000000000000000') — the two values are not equal although they render as the same text, so the difference lies in the value's exact characters (width/padding, whitespace, or a non-ASCII digit).
  divergence_key: service=lambda-microvms|GetMicrovmImageBuild|status=200->200|keys=$.buildId
  stub_path: EMPTY, owned_by: EMPTY (the only such cluster in this replay).
  Every other member of the 200 body agrees with the recording, and the step's status matches, so the cluster is this one member.

For contrast, in the same fixture at task#8 the corpus asserts the listing rows' buildId only as a shaped, non-asserted value and the emulator serves 'build-0000000000000000' / 'build-0000000000000001' there — i.e. the ids this service mints are a 'build-' prefix plus a zero-padded counter.

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): GetMicrovmImageBuild GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds/{buildId}, success 200, readonly; input buildId @httpLabel @required NonBlankString (@length 1..2048, pattern ^[^\s]+$); output buildId @required NonBlankString (same constraints), doc 'The build request ID.'; MicrovmImageBuildSummary$buildId targets the same shape.

Code sightings (orientation only): src/ops/get_microvm_image_build.rs:220-239 (lookup by input.build_id, then build_id: build.build_id.clone()); src/ops/list_microvm_image_builds.rs:245-277 (same field paged, cursored and projected into the listing row).

## Steps to Reproduce
1. mvm = client('lambda-microvms'); base = ListManagedMicrovmImages()['items'][0]['imageArn'].
2. CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<an IAM role arn>, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}) and wait for the version to settle.
3. rows = ListMicrovmImageBuilds(imageIdentifier=<imageArn>, imageVersion='1.0')['items']; bid = rows[0]['buildId'].
4. got = GetMicrovmImageBuild(imageIdentifier=<imageArn>, imageVersion='1.0', buildId=bid)['buildId'].
   AWS: got == bid exactly (the read echoes the identifier the URI addressed), and the value contains no whitespace (NonBlankString pattern ^[^\s]+$).
   Emulator: the identifier the read answers with is not byte-identical to the recorded identifier for that build at this step, although it renders as the same 'build-<digits>' text — assert with repr()/len() rather than by eye: `assert got == bid, (repr(got), repr(bid), len(got), len(bid))`.
5. Re-replay fixture lambdamicrovms-customer-image-lifecycle step task#9 to confirm the fix: the step must pass with $.buildId equal on both sides.
