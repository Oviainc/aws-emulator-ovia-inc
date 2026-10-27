Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.concurrent-association-operations
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.concurrent-association-operations|run#55|AssertionError|assert code == "NotFoundException", (
Issue-Commit: df009dd79b3a670d3c0c53c12bb5e37a115b862c

# AppSync DisassociateApi answers 501 NotImplemented for a never-created domain instead of 404 NotFoundException "Domain name not found.", and concurrent calls therefore share no per-call refusal

> _Note: this issue was identified against the code base at commit `df009dd79b3a670d3c0c53c12bb5e37a115b862c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateApi

## Description
Cause 1 of 1: AppSync `DisassociateApi` is an unimplemented scaffold handler, so the referential-integrity boundary on the custom-domain API association does not exist — a domain name that was never created with CreateDomainName is answered with an unmodeled 501 instead of the modeled 404, and because the refusal is produced by the scaffold rather than by a state lookup, concurrent calls cannot each independently report the not-found condition.

AWS: Smithy `DisassociateApi` = DELETE /v1/domainnames/{domainName}/apiassociation, success code 200 with an empty output; input `domainName` (DomainName, @required, @httpLabel, @length min 1 max 253, pattern ^(\*[\w\d-]*\.)?([\w\d-]+\.)+[\w\d-]+$). Modeled errors: AccessDeniedException (403), BadRequestException (400), ConcurrentModificationException (409), NotFoundException (404, documented "The resource specified in the request was not found. Check the resource, and then try again."), InternalFailureException (500). `NotImplemented` is modeled nowhere on this operation. A syntactically valid domainName that was never created by CreateDomainName can never carry an ApiAssociation, so AWS refuses the call with 404 NotFoundException; recorded behavior for the message: `Domain name not found.` — that whole sentence is FIXED text with no variable part (the domain name is not interpolated into it). Because the refusal is derived from state (the domain is absent), every concurrent caller gets the same 404/NotFoundException independently: there is no winner/loser split here, and ConcurrentModificationException (409) is NOT a legal answer when the domain does not exist. The recording issues three simultaneous DisassociateApi calls on one nonexistent, syntactically valid domain and requires all three to fail, each with code NotFoundException and that exact message.

Emulator: every DisassociateApi call returns error code `NotImplemented` (HTTP 501, message "operation DisassociateApi is scaffolded but not implemented in this emulator") regardless of input or state. Replay output: `AssertionError: expected NotFoundException for DisassociateApi on a nonexistent domain, got NotImplemented` (check_appsync_concurrent_association_operations.py:303). All three concurrent calls fail with that same unmodeled code, so the message assertion `msg == "Domain name not found."` is never even reached.

Why: a client cannot distinguish "this custom domain does not exist" from "the service is broken". `NotImplemented` is in no AppSync SDK error set, so a caller that branches on NotFoundException (to create the domain first, or to treat the disassociation as already done and proceed with teardown) takes the wrong branch and surfaces the 501 as an outage; retry-on-5xx logic will also loop on a condition that can never clear. Teardown/cleanup code that tolerates NotFoundException aborts instead.

Fix: the DisassociateApi handler in the appsync service crate — it must look the domain up in the DomainName/ApiAssociation state and return NotFoundException/404 with the fixed message "Domain name not found." when the domain (and hence its association) is absent, rather than the scaffold's not_implemented answer; the scaffold marker in that file must be removed so the state lookup, not operation dispatch, decides the outcome (which is also what makes the refusal independent per concurrent call).

Related open issue (does NOT cover this check): 20261025T041655Z-appsync-event-api-domain-control-plane-no-identity.md, Cause 3, describes AssociateApi/GetApiAssociation/DisassociateApi answering NotImplemented for a never-created domain, but it is grounded only in appsync.reject-api-domain-association-mismatched-state and does not carry this check's replay key, nor the concurrent/per-call-independence and exact-message contract recorded here. Fix the two together; this issue exists so appsync.concurrent-association-operations is re-replayed before the cause is closed. No open or fixed stub issue names DisassociateApi.

Traces: appsync.concurrent-association-operations (the earlier merged-API association race assertions in that check pass; it fails first at the DisassociateApi boundary).

## Evidence
appsync.concurrent-association-operations (verdict: mismatch):
  expected: three concurrent `disassociate_api(domainName="<unique>.com")` calls on a syntactically valid domain name that was NEVER passed to CreateDomainName -> all three fail, each with error code "NotFoundException" (HTTP 404) and message exactly "Domain name not found." (fixed text, no interpolated values).
  actual:   AssertionError: expected NotFoundException for DisassociateApi on a nonexistent domain, got NotImplemented (check_appsync_concurrent_association_operations.py:303, via run.py:176). All three calls return error code `NotImplemented`, HTTP 501, message "operation DisassociateApi is scaffolded but not implemented in this emulator".
  Smithy: DisassociateApi DELETE /v1/domainnames/{domainName}/apiassociation, success 200; errors = AccessDeniedException(403), BadRequestException(400), ConcurrentModificationException(409), InternalFailureException(500), NotFoundException(404); input domainName required @length 1..253 pattern ^(\*[\w\d-]*\.)?([\w\d-]+\.)+[\w\d-]+$. NotImplemented is not a modeled error and 501 is not a modeled status.
  Handler: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/disassociate_api.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4) and its handle() body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("DisassociateApi"))` (line 159), so the 501 is returned before any state lookup.

## Steps to Reproduce
1. Boot the emulator and point an AppSync client at it.
2. Pick a syntactically valid domain name that was never passed to CreateDomainName (e.g. "<unique-token>.com", matching ^(\*[\w\d-]*\.)?([\w\d-]+\.)+[\w\d-]+$).
3. Call appsync disassociate_api(domainName=<that domain>). Observe error code NotImplemented, HTTP 501, where AWS answers NotFoundException, HTTP 404, message "Domain name not found.".
4. Fire three disassociate_api calls on that same domain concurrently. Observe that all three fail with NotImplemented/501; AWS fails all three independently with NotFoundException/404 and that same message (never ConcurrentModificationException, since the domain does not exist).
5. Replay appsync.concurrent-association-operations.
