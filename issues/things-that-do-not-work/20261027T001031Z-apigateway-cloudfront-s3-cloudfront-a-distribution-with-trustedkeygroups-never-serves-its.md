Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive|_poll_http_get#29|Timeout|time.sleep(interval_s)
Issue-Commit: 4bb3c01901698ef95426c2799d1398daad63aada

# cloudfront: a distribution with TrustedKeyGroups never serves its S3 origin, so neither the unsigned 403 nor the signed 200 ever arrives

> _Note: this issue was identified against the code base at commit `4bb3c01901698ef95426c2799d1398daad63aada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: a distribution created over an S3 origin with an OAC and a required trusted key group serves nothing at its domain name.
AWS: recorded behavior only (the cloudfront model types no edge behaviour) — once the distribution reaches Status `Deployed`, a viewer request to https://<DomainName>/<key> without a CloudFront signature is answered 403 while the same URL carrying a valid CloudFront URL signature for a public key in the configured key group is answered 200 with the exact S3 object bytes.
Emulator: the check ends as `Check timed out after 60.0s`; the distribution never becomes serviceable, so neither poll (403 for the unsigned URL, 200+bytes for the signed URL) ever observes its condition. (Hypothesis: the same edge-serving gap as the already-open 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md, which does not carry this check's replay key — this check additionally covers the TrustedKeyGroups/signed-URL leg, which no open issue names.)
Why: a client cannot validate CloudFront signed-URL protection at all: the enforcement (unsigned -> 403) and the grant (valid signature -> object bytes) are both unobservable, so a misconfigured key group looks the same as a correct one.
Fix: the CloudFront data-plane/edge handling for a distribution's domain name — origin fetch from the S3 origin under an OAC, plus the viewer-request trusted-key-group signature check that must precede it.
Traces: apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive.

## Evidence
`Error: Check timed out after 60.0s`. The check creates an OAC, an RSA public key and a key group, creates a distribution whose DefaultCacheBehavior sets TrustedKeyGroups{Enabled:true}, puts the OAC bucket policy, waits for Deployed, then polls GET https://<DomainName>/protected.txt for 403 (unsigned) and for 200 + the exact body (CloudFrontSigner-signed). Neither condition is ever met.

## Steps to Reproduce
Replay apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive: private bucket + object, CreateOriginAccessControl, CreatePublicKey + CreateKeyGroup, CreateDistribution with an S3OriginConfig + OriginAccessControlId and DefaultCacheBehavior.TrustedKeyGroups enabled, PutBucketPolicy for the OAC, wait for Deployed, then GET the object URL unsigned and with a CloudFront URL signature.
