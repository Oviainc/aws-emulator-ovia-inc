Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.catalog-logo-blob-is-stored-and-served-as-a-real-image ecr-public.repository-catalog-data-base64-logo-round-trip
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public.catalog-logo-blob-is-stored-and-served-as-a-real-image|_fetch_until_served#11|AssertionError|assert status == 200, (
Issue-Key: ecr-public.repository-catalog-data-base64-logo-round-trip|_poll_until#8|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# ECR Public: the logoImageBlob accepted by PutRepositoryCatalogData is never served at the minted logoUrl — an anonymous GET of that URL answers 404

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRepositoryCatalogData

## Description
Behavioral replay of the simparity-behavioral-aws-v1 ecr-public suite (85/92 passed; ecr-public 84/91, the emulator IS serving and ecr-public is bootstrapped as service-com-amazonaws-ecrpublic-spencerfrontendservice, so this is not a bootstrap gap) leaves ONE net-new root cause after excluding every check the report marked ALREADY FILED or OWNED by an open issue.

NOTE on an overlapping open issue: cause 2 of the open issue 20261023T055032Z-ecr-public-getauthorizationtoken-is-not-authorization-gated-stored.md describes the same operation and the same requirement, but it recorded the symptom as a never-converging fetch poll (its replay keys for these two checks are `_fetch_until_served#11|Timeout|time.sleep(2.0)` and `_poll_until#8|Timeout|time.sleep(interval_s)`). The emulator's behavior has since changed — the GET now terminates promptly with HTTP 404 instead of hanging — so the replay report no longer binds these two checks to that issue and they are refiled here with their current keys. If that issue's cause 2 is fixed, re-replay the two checks listed below to confirm this cause closes with it.

=== Cause 1 of 1: the logo bytes accepted by PutRepositoryCatalogData are not stored and nothing is served at the logoUrl the service minted for them ===
AWS: Smithy RepositoryCatalogDataInput.logoImageBlob is a blob, @length 0..512000, documented "The base64-encoded repository logo payload", and is INPUT-only: the output shape RepositoryCatalogData models no logoImageBlob and instead carries logoUrl -> ResourceUrl @length 0..2048, documented "The URL that contains the logo that's associated with the repository." The uploaded bytes are therefore turned into a hosted asset, and the minted URL is the only way a client can observe them. Recorded behavior only for the hosting leg (it is outside the modeled API surface): an anonymous HTTPS GET of the minted logoUrl answers 200 with the byte-identical payload that was uploaded — same length, same PNG signature, same IHDR width/height, served with Content-Type image/png — and a subsequently accepted replacement blob is published at a DIFFERENT logoUrl that serves the new bytes while the description is left untouched.
Emulator: PutRepositoryCatalogData accepts the blob and returns a logoUrl, and GetRepositoryCatalogData reports the same logoUrl on the target's own published host (the harness's published_url_mismatch host check passes, so the URL is routed to the emulator itself and this is not an egress/announced-address problem). Every assertion before the fetch passes: logoImageBlob is not echoed, logoUrl is present, the description round-trips. But an anonymous GET of that URL answers HTTP 404 and keeps answering 404 for the whole budget (120s in one check, 60s in the other): the path under /logos/<repository-name>/<asset-hash> is not claimed by any route, so the bytes were counted and discarded rather than persisted. Because the URL never answers 200, the checks never reach the downstream legs (byte identity, Content-Type image/png, the undersized-logo refusal leaving the stored asset and URL intact, and the accepted replacement minting a new URL serving the new bytes) — those remain unverified.
Why: logoImageBlob appears in no response, so fetching the minted URL is the ONLY way a client can confirm the logo was stored or display it. Any tool that uploads a logo and then verifies or renders it (a gallery preview, a publish pipeline, an IaC post-condition) gets a permanently 404ing URL against the emulator while AWS serves the image. Worse, the emulator reports success and hands back a well-formed URL, so the failure surfaces only at the HTTP fetch, far from the call that caused it; and since an emulator has no propagation delay, a 404 that never clears means the asset does not exist, not that publishing is slow.
Fix: the PutRepositoryCatalogData handler together with the repository-catalog state must persist the logo bytes keyed by the URL it mints, and the emulator's HTTP front door must claim that path and serve the stored bytes verbatim with the image content type; a replacement blob must mint a distinct URL serving the new bytes, and a refused blob must leave both the stored bytes and the existing logoUrl unchanged.
Traces: ecr-public.catalog-logo-blob-is-stored-and-served-as-a-real-image, ecr-public.repository-catalog-data-base64-logo-round-trip.

## Evidence
Replay: simparity-behavioral-aws-v1, total 85/92 passed; ecr-public 84/91 with 7 failures, 5 of which the report marked OWNED by open issues and are excluded here.

[mismatch] ecr-public.catalog-logo-blob-is-stored-and-served-as-a-real-image — CreateRepository, then PutRepositoryCatalogData with a hand-built 64x64 truecolour PNG as logoImageBlob plus a description; GetRepositoryCatalogData returns the logoUrl (and passes ctx.published_url_mismatch, i.e. the URL points at the emulator's own published host). The check then GETs that URL anonymously every 2s for 120s and fails at check_ecr_public_catalog_logo_blob_is_stored_and_served_as_a_real_image.py line 92:
  AssertionError: the logo URL the service minted never served the asset within 120.0s: 404 for 'https://public.ecr.aws/logos/sim2real-behav-ecrpub-repo-b00bb4889227/50a970f0a1af8b7580138b13595f0a9a776c51a645d22a4841c46e78a4d36139'
(the repository name and the asset hash in that URL are per-run values minted by the emulator; the requirement is only that whatever URL the service minted answers 200 with the uploaded bytes.)

[mismatch] ecr-public.repository-catalog-data-base64-logo-round-trip — same shape of call (CreateRepository, PutRepositoryCatalogData with a 64x64 RGB PNG and nothing else), logoUrl is surfaced by GetRepositoryCatalogData within its 30s poll, then the fetch poll (3s interval, 60s budget) never gets a body and fails at check_ecr_public_repository_catalog_data_base64_logo_round_trip.py line 103:
  AssertionError: condition not met within 60s (last=None)
  (its _try_fetch swallows the HTTPError and returns None, so the None is the same 404 the other check reports verbatim.)

Expected in both: HTTP 200, body byte-identical to the uploaded PNG (same length, b'\x89PNG\r\n\x1a\n' signature, IHDR reading 64x64), Content-Type 'image/png'. Observed: HTTP 404 for the whole budget. No AWS error code is involved — this is the asset-hosting leg, not a modeled API response; the PutRepositoryCatalogData and GetRepositoryCatalogData calls themselves answer 200 with a correctly shaped RepositoryCatalogData (logoImageBlob absent, logoUrl present).

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, and replay the simparity-behavioral-aws-v1 ecr-public suite (run_behavioral_suite).
2. Replay ecr-public.catalog-logo-blob-is-stored-and-served-as-a-real-image and ecr-public.repository-catalog-data-base64-logo-round-trip.
3. By hand: against the us-east-1 ecr-public endpoint, CreateRepository with a fresh name, then PutRepositoryCatalogData with catalogData.logoImageBlob set to a valid 64x64 PNG (both axes must be at or above the service's 60px floor) and a description.
4. Read catalogData.logoUrl back from GetRepositoryCatalogData, then issue an anonymous HTTP GET for that URL routed at the emulator. It must answer 200 with a body byte-identical to the PNG that was uploaded and Content-Type image/png. The emulator answers 404 indefinitely.
5. Then exercise the legs the 404 currently hides: PutRepositoryCatalogData a 32x32 PNG (must be refused 400 InvalidParameterException) and confirm logoUrl, the description and the served bytes are all unchanged; then PutRepositoryCatalogData a different valid 64x64 PNG and confirm GetRepositoryCatalogData reports a NEW logoUrl that serves the new bytes while the description survives.
