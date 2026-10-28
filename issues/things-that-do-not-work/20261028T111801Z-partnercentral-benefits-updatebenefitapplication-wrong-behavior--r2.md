Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fa255a5b72d32034e1d300bb9323da879d85edac

# Idempotency compares the raw Identifier but looks the token up per application, so a true retry spelled with the ARN is refused while a different application's write is accepted

_Filed: 2026-10-28T11:18:01Z_
_Operation: UpdateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fa255a5b72d32034e1d300bb9323da879d85edac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `ClientToken`-carrying write of this crate states the same rule in its handler doc: a request repeating a spent token is the SAME request and is replayed, while "a request carrying that token with any different member is a different request and is refused with the modelled `ConflictException` (HTTP 409)". The implementation contradicts that rule in both directions, because the request identity (`request_fingerprint`) and the place the token is remembered disagree about what identifies the request.

1. **A true retry is refused.** The fingerprint hashes the RAW `Identifier` string the request carried (`fingerprint_value(&input.identifier)`), but the application the token is looked up on is resolved through `state::resolve_application_id`, which this crate documents as accepting "either form the modelled `BenefitApplicationIdentifier` allows (`^(arn:.+|benappl-[0-9a-z]{14})$`)" for the same record. So a retry of one and the same update on one and the same application, spelled with the application ARN where the first call used the bare `benappl-…` id (both forms the model admits, both forms the service serves), finds the applied write, compares a different fingerprint, and is answered 409 `ConflictException` with the idempotent-parameter-mismatch sentence instead of replaying the first call's `Id`/`Arn`/`Revision`. The crate already owns the normalisation helper that would make the two spellings one request; the fingerprint path is the one path that does not call it.

2. **A genuinely different request is served.** `BenefitApplication::applied_write` scans only the applied writes of the application the request named, so a token spent on application A is invisible when the request names application B. `Identifier` is therefore in the fingerprint but can never differ on a record that was found: per the handler doc an update of B carrying A's spent token must be refused 409 (the member `Identifier` differs), yet it is applied and answered 200. The same construction, with `Catalog` instead of `Identifier`, is in `CreateBenefitApplication`: its token index is per catalog (`catalog.client_tokens`), so the `Catalog` entry of its fingerprint is likewise unreachable and the same `ClientToken` replayed in the other seeded catalog mints a SECOND application instead of the documented 409.

The same lookup/fingerprint pair is used by `AmendBenefitApplication` (src/ops/amend_benefit_application.rs:488-491, :542-545) and `CancelBenefitApplication` (src/ops/cancel_benefit_application.rs:189-192, :230-233), and `RecallBenefitApplication`'s header states the rule explicitly for the case the code cannot detect ("a recall carrying that token with a different `Reason` or naming another application is a different request and is refused with the modelled `ConflictException`") — a recall of another application with that token is served instead.

## Evidence

The rule the crate states — src/ops/update_benefit_application.rs:24-31:
    //! Idempotency: `ClientToken` identifies the REQUEST, so a retry carrying the token of an
    //! update that already applied changes nothing a second time and answers with the
    //! `Id`/`Arn`/`Revision` that update reported, while an update carrying that token with
    //! any different member is a different request and is refused with the modelled
    //! `ConflictException` (HTTP 409) — see [`crate::state::replayed_write`] and
    //! [`crate::state::IDEMPOTENT_PARAMETER_MISMATCH`].

The identity it compares includes the RAW identifier — src/ops/update_benefit_application.rs:239-242:
            (
                "Identifier",
                Some(crate::state::fingerprint_value(&input.identifier)),
            ),

The record it compares against is looked up on the RESOLVED application — src/ops/update_benefit_application.rs:300 and :323-326:
        let Some(application) = store.application_mut(&input.catalog, &input.identifier) else {
...
        match crate::state::replayed_write(
            application.applied_write(Self::NAME, &input.client_token),
            &fingerprint,
        ) {

and that lookup never leaves the one application — src/state.rs:1185-1196:
    /// The write `operation` already applied to this application under `client_token`,
    /// if this application still holds a record of it …
    /// The operation is part of the key: a `ClientToken` identifies ONE request, so the
    /// same token spent on a different operation is a different request rather than a
    /// retry of this one.
    pub fn applied_write(&self, operation: &str, client_token: &str) -> Option<&AppliedWrite> {
        self.applied_writes
            .iter()
            .find(|applied| applied.operation == operation && applied.client_token == client_token)

The helper the fingerprint path skips, with the crate's own statement that the two spellings name ONE record — src/state.rs:1518-1537:
    /// The application `identifier` names inside `catalog`, accepting either form the
    /// modelled `BenefitApplicationIdentifier` allows (`^(arn:.+|benappl-[0-9a-z]{14})$`).
    …
    pub fn resolve_application_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        if identifier.starts_with("arn:") {
            return self.arns.get(identifier)…

The same shape in the create, with `Catalog` as the unreachable member — src/ops/create_benefit_application.rs:8-14 ("a create carrying that token with any different member is a different request and is refused with the modelled `ConflictException` (HTTP 409)"), :258-262 (`("Catalog", Some(crate::state::fingerprint_value(&input.catalog)))`) and :349 (`if let Some(spent) = catalog.client_tokens.get(&input.client_token) {`, where `catalog` is `store.catalogs.get(&input.catalog)`).

The recall header, stating the undetectable half outright — src/ops/recall_benefit_application.rs:13-19:
    //! `ClientToken` is this request's idempotency token, so a retry carrying the token of
    //! the recall that is still in effect answers success and changes nothing … while a
    //! recall carrying that token with a different `Reason` or naming another application
    //! is a different request and is refused with the modelled `ConflictException`

Test coverage: no test pins either case. tests/all/client_token_idempotency.rs:16-21 enumerates exactly what its rows vary ("WHICH `ClientToken`-carrying write is replayed …, which members the write carried, the FORM and LENGTH of the token …, what happened to the application in between, and which operation the token was spent on") — the FORM of the `Identifier` and the application/catalog named are not among them; tests/all/client_token_idempotency.rs:306-310 pins only operation-scoping and case-sensitivity; every row of tests/all/idempotent_parameter_mismatch.rs:314-480 reuses the one `"Identifier": id` in the one `CATALOG`. So neither answer is pinned with a stated reason.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_application_lifecycle.rs:90 does.
2. CreateBenefitApplication in catalog `AWS` for an ACTIVE benefit; keep the returned `Id` (benappl-…) and `Arn`.
3. UpdateBenefitApplication with Catalog=AWS, Identifier=<the bare Id>, Revision="1", ClientToken="t", Name="x" → 200, Revision "2".
4. Replay the identical update but spell Identifier as the application's `Arn` (the other form the modelled pattern admits and the form state::resolve_application_id resolves to the same record), same ClientToken="t", same Revision="1", same Name="x". Observe 409 ConflictException carrying state::IDEMPOTENT_PARAMETER_MISMATCH, where the handler doc requires the first call's Id/Arn/Revision to be replayed.
5. CreateBenefitApplication a second application B in catalog `AWS`. Call UpdateBenefitApplication with Catalog=AWS, Identifier=<B's Id>, Revision="1", ClientToken="t" (the token already spent on A), Name="x". Observe 200 and B written, where the handler doc requires 409 because the member `Identifier` differs from the request the token was spent on.
6. CreateBenefitApplication with ClientToken="t2" in catalog `AWS`, then send the same create body with ClientToken="t2" and Catalog=`Sandbox`. Observe 200 and a second application, where src/ops/create_benefit_application.rs:8-14 requires 409 because the member `Catalog` differs.
7. Repeat step 5 against AmendBenefitApplication, CancelBenefitApplication and RecallBenefitApplication for the same answer from src/ops/amend_benefit_application.rs:542-545, src/ops/cancel_benefit_application.rs:230-233 and src/ops/recall_benefit_application.rs:229-239.
