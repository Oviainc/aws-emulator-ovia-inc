Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.deleting-a-configuration-set-leaves-no-dangling-identity-default-reference
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.deleting-a-configuration-set-leaves-no-dangling-identity-default-reference|run#12|AssertionError|assert reference == config_set, (
Issue-Commit: 48883a331e1fae405ebe84b115d9b023bc18cbad

# sesv2: DeleteConfigurationSet cascades into identities and clears their default configuration-set reference, where AWS leaves the dangling name in place

> _Note: this issue was identified against the code base at commit `48883a331e1fae405ebe84b115d9b023bc18cbad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfigurationSet

## Description
One net-new behavioral root cause in the sesv2 configuration-set / email-identity plane. It is distinct from the open sesv2 issues that already own other failures of this suite (notably 20261028T025357Z-sesv2-3-net-new-behavioral-root-causes-deleteemailidentity.md, which owns the DeleteEmailIdentity message bracketing and the SendEmail list-management causes, and 20261026T065823Z-sesv2-the-notfoundexception-message-for-a-missing-configuration.md, which owns the configuration-set not-found SENTENCE). This cause is about DELETE-time state propagation, not about a message or a status.

=== Cause 1 of 1: DeleteConfigurationSet clears every identity's ConfigurationSetName that named the deleted set ===
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) models DeleteConfigurationSet as DELETE /v2/email/configuration-sets/{ConfigurationSetName} -> 200 with an empty response body, errors BadRequestException (400), ConcurrentModificationException (500), NotFoundException (404), TooManyRequestsException (429). The model says nothing about referrers, so the referential behavior is recorded behavior only: an identity's default configuration set, set with PutEmailIdentityConfigurationSetAttributes (PUT /v2/email/identities/{EmailIdentity}/configuration-set) and read back as GetEmailIdentity.ConfigurationSetName (an unconstrained ConfigurationSetName string, not a validated foreign key), is NOT a reference AWS maintains. Deleting the configuration set succeeds unconditionally (no refusal for 'still in use') and leaves the identity's ConfigurationSetName reporting the now-dead set name verbatim; the dangling name is inert but still present, and is only removed when the client itself clears it by calling PutEmailIdentityConfigurationSetAttributes with no ConfigurationSetName (after which GetEmailIdentity reports no ConfigurationSetName). The reverse direction is likewise non-cascading: DeleteEmailIdentity leaves the configuration set it named alive and resolvable through GetConfigurationSet.
Emulator: the delete itself succeeds and the set becomes correctly unusable (GetConfigurationSet and PutConfigurationSetSendingOptions on the dead name both refuse NotFoundException, as AWS does), but the handler also walks every identity and nulls the reference: after DeleteConfigurationSet(<set>), GetEmailIdentity(<identity>) reports ConfigurationSetName=None where AWS reports ConfigurationSetName=<the deleted set's name> (the value the client previously PUT). The cascade is explicit in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_configuration_set.rs lines 174-182, which iterate store.identities.values_mut() and assign identity.configuration_set_name = None for each identity whose reference equals the deleted name.
Why: a client that reads GetEmailIdentity to discover which configuration set an identity defaults to gets a different answer against the emulator than against AWS after an unrelated configuration-set deletion. Reconciliation and drift-detection tooling that looks for dangling references (in order to recreate the set, or to report the identity as mis-configured) sees a clean, unreferenced identity on the emulator and never fires; conversely, code that relies on the dangling name surviving so it can recreate the set under the same name and have the identity keep defaulting to it silently loses the association. The emulator is also internally inconsistent with the field's real nature: it treats ConfigurationSetName as a maintained foreign key on delete, while on write it is an unchecked string.
Fix: the DeleteConfigurationSet handler in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_configuration_set.rs — the delete must remove only the configuration-set record (with its options, tags and event destinations) and must not touch any identity record; the identity's configuration_set_name field in the sesv2 state module stays as the client last wrote it and is cleared only by PutEmailIdentityConfigurationSetAttributes with no ConfigurationSetName.
Traces: sesv2.deleting-a-configuration-set-leaves-no-dangling-identity-default-reference.

## Evidence
Suite: 49/107 passed. The only surviving failing check (not ALREADY FILED, not OWNED):

[mismatch] sesv2.deleting-a-configuration-set-leaves-no-dangling-identity-default-reference — check_sesv2_deleting_a_configuration_set_leaves_no_dangling_identity_default_reference.py line 49:
AssertionError: DeleteConfigurationSet(sim2real-behav-cs-referenced-36a54d7f94ec) cleared or refused the delete: GetEmailIdentity(sim2real-behav-csref-93ba9e373390@example.com) now reports ConfigurationSetName=None, but real AWS is measured to leave the dangling reference 'sim2real-behav-cs-referenced-36a54d7f94ec' in place instead.

The set name and the identity address are per-run unique values generated by the check; the requirement is that after the delete GetEmailIdentity still reports the SAME ConfigurationSetName string the client previously PUT, whatever that string is.

Beats that already matched before the failing assertion: CreateConfigurationSet + CreateEmailIdentity succeeded; PutEmailIdentityConfigurationSetAttributes associated the set and GetEmailIdentity reported it (positive control); PutConfigurationSetSendingOptions worked while the set existed; DeleteConfigurationSet returned success; GetConfigurationSet on the deleted name no longer resolved. Only the identity's surviving reference diverges.

Emulator source: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_configuration_set.rs:174-182 ('An identity's default configuration set is a reference to this record, so the reference goes with it' — the loop that sets identity.configuration_set_name = None).

## Steps to Reproduce
Replay the behavioral suite and re-run sesv2.deleting-a-configuration-set-leaves-no-dangling-identity-default-reference.

Manually:
1. CreateConfigurationSet(ConfigurationSetName=CS).
2. CreateEmailIdentity(EmailIdentity=ADDR) for an email-address identity (no verification needed).
3. PutEmailIdentityConfigurationSetAttributes(EmailIdentity=ADDR, ConfigurationSetName=CS); GetEmailIdentity(ADDR) reports ConfigurationSetName=CS.
4. DeleteConfigurationSet(ConfigurationSetName=CS) — succeeds (200, empty body), and GetConfigurationSet(CS) / PutConfigurationSetSendingOptions(CS, ...) then refuse 404 NotFoundException.
5. GetEmailIdentity(ADDR): expect ConfigurationSetName=CS to still be reported (the inert dangling reference AWS leaves in place); observe the emulator reporting no ConfigurationSetName at all.
6. PutEmailIdentityConfigurationSetAttributes(EmailIdentity=ADDR) with no ConfigurationSetName: this is the only call that may clear it; GetEmailIdentity(ADDR) then reports no ConfigurationSetName.
7. Reverse direction (already correct, keep it so): create CS2 + ADDR2, associate them, DeleteEmailIdentity(ADDR2) — GetEmailIdentity(ADDR2) refuses NotFoundException and GetConfigurationSet(CS2) still resolves.
