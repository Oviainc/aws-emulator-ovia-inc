Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.validate-pull-through-cache-rule-not-found ecr.registry-scanning-configuration-enforced-on-push
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.validate-pull-through-cache-rule-not-found|run#12|AssertionError|assert validate_resp["isValid"] is True, (
Issue-Key: ecr.registry-scanning-configuration-enforced-on-push|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7

# ECR: ValidatePullThroughCacheRule reports a registered rule as invalid, and registry-level scan-on-push never produces a scan (2 causes)

> _Note: this issue was identified against the code base at commit `b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidatePullThroughCacheRule

## Description
TWO net-new root causes in ECR behavior (not message wording). Related already-open issue 20261024T014332Z-ecr-asynchronous-effects-never-occur-and-several-enforcement.md covers other asynchronous ECR gaps but does not carry either replay key below; fixed issue 20261023T153036Z-ecr-validatepullthroughcacherule-marks-a-complete-rule-invalid-citing.md claimed cause 1's symptom was repaired and fixed issue 20261025T025458Z-ecr-basic-image-scanning-can-never-reach-complete.md claimed the scan plane was repaired.

Cause 1 of 2 — ValidatePullThroughCacheRule answers isValid=false for a rule this same emulator just created.
AWS: Smithy (ecr ValidatePullThroughCacheRule) outputs isValid (IsPTCRuleValid, @default false) plus an optional failure (PTCValidateFailure) and models PullThroughCacheRuleNotFoundException for an unregistered prefix. A rule registered for a reachable public upstream validates with isValid=true and no failure reason; the not-found rejection is reserved for a prefix no rule covers.
Emulator: after CreatePullThroughCacheRule(ecrRepositoryPrefix=<prefix>, upstreamRegistry="quay", upstreamRegistryUrl="quay.io") succeeds, ValidatePullThroughCacheRule on that very prefix answers HTTP 200 with isValid=false.
Why: validation is the documented pre-flight for a pull-through cache rule, so every client that gates deployment on isValid concludes a correctly-registered rule is broken; the two outcomes (registered-and-valid vs not-found) become indistinguishable from each other in effect.
Fix: the ValidatePullThroughCacheRule handler / pull-through-cache rule state — a rule present in state with a public upstream must validate true rather than reporting an emulator-local reachability failure.
Traces: ecr.validate-pull-through-cache-rule-not-found (its not-found leg and refusal message passed; the positive control is what failed).

Cause 2 of 2 — registry-level SCAN_ON_PUSH produces no scan at all for a newly pushed image.
AWS: Smithy models PutRegistryScanningConfiguration (scanType, rules with scanFrequency SCAN_ON_PUSH and repositoryFilters) and DescribeImageScanFindings with imageScanStatus. Recorded behavior: with registry scanning set to BASIC/SCAN_ON_PUSH matching the repository, a pushed image is scanned automatically without any StartImageScan call and DescribeImageScanFindings reaches a terminal imageScanStatus.status (COMPLETE, FAILED or UNSUPPORTED_IMAGE) on its own.
Emulator: the configuration write and read-back are accepted, but after the push DescribeImageScanFindings never reaches any terminal status within the replay budget — the check polls to the 60 s replay cap and is killed. Hypothesis: no scan record is created by the push path at all (the probe treats ScanNotFoundException as "keep waiting"), so no poll length would satisfy it.
Why: scan-on-push is the enforcement a registry-wide scanning configuration exists to deliver; a client that pushes and then waits for findings hangs forever and never learns the image's vulnerability state.
Fix: the PutImage/push path's scan-on-push trigger against the registry scanning configuration, and the scan state that DescribeImageScanFindings reads.
Traces: ecr.registry-scanning-configuration-enforced-on-push.

## Evidence
Cause 1: ecr.validate-pull-through-cache-rule-not-found — [mismatch] AssertionError at check line 51: "expected isValid=True for a registered rule with a reachable public upstream, got False". The same check's unregistered-prefix leg (PullThroughCacheRuleNotFoundException plus its exact message) passed, so the failure is specifically the registered-rule positive control.
Cause 2: ecr.registry-scanning-configuration-enforced-on-push — [mismatch] "Check timed out after 60.0s". The check asserts GetRegistryScanningConfiguration echoes BASIC + SCAN_ON_PUSH + the repository filter (those reads are what it does before the push) and then polls DescribeImageScanFindings for a terminal imageScanStatus.status in {COMPLETE, FAILED, UNSUPPORTED_IMAGE}; no terminal status was ever observed.

## Steps to Reproduce
Cause 1: CreatePullThroughCacheRule(ecrRepositoryPrefix=<unique prefix>, upstreamRegistryUrl="quay.io", upstreamRegistry="quay"), then ValidatePullThroughCacheRule(ecrRepositoryPrefix=<that prefix>) and read isValid — it is false.
Cause 2: CreateRepository(<repo>); PutRegistryScanningConfiguration(scanType="BASIC", rules=[{scanFrequency:"SCAN_ON_PUSH", repositoryFilters:[{filter:"<repo>*", filterType:"WILDCARD"}]}]); push an image (InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload/PutImage); then poll DescribeImageScanFindings(repositoryName=<repo>, imageId={imageDigest:<pushed digest>}) — it never reports a terminal imageScanStatus.
