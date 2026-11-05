Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively|_poll#14|Timeout|time.sleep(interval_s)
Issue-Commit: 6fd31b9d6eba67a77ab8adb40614f3180f2a16f5

# secretsmanager: ListSecrets `description` / `all` filter matching never converges on the documented secret set — the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `6fd31b9d6eba67a77ab8adb40614f3180f2a16f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecrets

## Description
Cause 1 of 1: ListSecrets does not serve the secret set that the modelled per-key filter semantics require for the text-searching filter keys (`description` and `all`) alongside a scoping `name` filter, so a client that polls for the filtered answer never sees it converge. Failing check id: secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively.

AWS: Smithy (com.amazonaws.secretsmanager#ListSecrets, Filter$Key, FilterNameStringType = description | name | tag-key | tag-value | primary-region | owning-service | all) documents the matching rule per key: `description` is a "Prefix match, not case-sensitive"; `name`, `tag-key`, `tag-value`, `primary-region` and `owning-service` are case-sensitive prefix matches; `all` "[b]reaks the filter value string into words and then searches all attributes for matches. Not case-sensitive." Filter$Values additionally documents the `!` negation prefix, and Filters is a list of 0..10 filters that are ANDed. Concretely, for a secret described "Payment gateway credentials" and a secret described "reporting Database login": description=Payment, description=payment, description=gateway, description=DATABASE and description=cred each select the matching secret; all=gateway, all=GATEWAY, all=<a tag value>, all=<a tag key>, all=<a prefix shared by a description word and a tag value> and all=<a word of the name> select every secret any of those attributes matches; a two-word `all` value selects only secrets matching BOTH words; and a `name` filter whose value differs only in case selects nothing, because `name` stays case-sensitive. ListSecrets is a 200 read — there is no refusal involved in any of these queries.

Emulator: the check creates three secrets (two with descriptions, two with a shared tag key, one with a tag value), scopes every query by a `name` prefix only its own secrets carry, and then polls each filtered query with a bounded poll until it returns the expected name set. The replay never converges: the check reaches the harness's 60 s cap and is reported as `[mismatch] secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively — Error: Check timed out after 60.0s`, so at least one of the polled filter queries kept answering a set other than the documented one for the whole budget (either the name-scoped baseline of the three created secrets, or one of the eleven description/all cases). No error response is involved; the operation answers 200 with the wrong SecretList membership.

Why: a client that lists secrets by `description` or `all` (the console's "find secrets" search and any SDK caller reproducing it) gets a filtered listing that silently omits or over-reports secrets. Because the answer is a 200 with a wrong SecretList rather than a refusal, the client cannot tell a wrong filter result from an account that genuinely holds no matching secret, and a polling client (the normal pattern, since ListSecrets is documented as eventually consistent) waits forever instead of failing.

Fix: the ListSecrets filter-matching path — the shared filter matcher used by the listing (and by the batched read, which shares the `Filter` shape) and the handler that applies and pages it. Hypothesis (neither the model nor the recording states which of these is the one that diverges, since the only recorded symptom is the non-convergence): the candidates are (a) the set of attributes the `all` key searches — the matcher searches the secret's Region in addition to name/description/tag keys/tag values, which is not one of the attributes AWS's search covers, so `all` can over-select; (b) the word-splitting/prefix rule used for `description` and `all` on hyphenated values (tag keys and generated secret names are hyphenated, and whether the whole value or each of its alphanumeric runs is the matched unit decides those cases); and (c) the no-MaxResults page/NextToken path the SDK paginator walks for a filtered query, including the empty-result page. Any fix must keep the listing and the batched read agreeing about what one filter means.

Traces: the one failing check above is the example; re-replay it to confirm the fix (it must complete well inside the 60 s cap, not merely stop timing out).

## Evidence
Suite report, service heading `### secretsmanager`:

  #### [mismatch] secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively
  Error: Check timed out after 60.0s

Expected (AWS, per Filter$Key in com.amazonaws.secretsmanager#ListSecrets): each of these name-scoped queries returns its documented set, each in a 200 response — description={Payment|payment|gateway|cred} -> the secret described "Payment gateway credentials"; description=DATABASE -> the secret described "reporting Database login"; all={gateway|GATEWAY|<tag value>} -> the secrets carrying that attribute word; all=<shared tag key> -> both tagged secrets; all=<prefix shared by a description word and a tag value> -> both of those secrets; all=<name word> -> the secret whose name carries it; all="<word-a> <word-b>" -> only secrets matching BOTH words (here: empty); and a `name` value differing only in case -> empty, because `name` is case-sensitive.

Actual (emulator): at least one of those queries keeps answering a different SecretList for the whole 60 s budget, so the check's bounded poll never converges and the check is cut off at the cap. No refusal, no error code — a 200 with wrong membership.

Related but NOT covering this: no open issue addresses ListSecrets filter matching. The closest secretsmanager listings/filters issues are about refusal identity for malformed filters (20261024T190718Z-secretsmanager-secrets-manager-refusals-pick-the-wrong-exception-code.md, which owns secretsmanager.list-secrets-invalid-filter-key-value-and-page-size-rejected) and about BatchGetSecretValue request form (20261025T173233Z-secretsmanager-secrets-manager-batchgetsecretvalue-request-form-refusal-text-rules.md); both are refusal-path issues, neither covers which secrets a valid description/all filter selects.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (the emulator is bootstrapped; secretsmanager answers 132/191 checks).
2. Replay check `secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively`.
3. Equivalent manual reproduction against the emulator's secretsmanager endpoint:
   a. CreateSecret NAME=<base>-app-1, SecretString=<any>, Description="Payment gateway credentials", Tags=[{Key: <base>-tier, Value: web}].
   b. CreateSecret NAME=<base>-app-2, SecretString=<any>, Description="reporting Database login" (no extra tags).
   c. CreateSecret NAME=<base>-db-1, SecretString=<any>, no Description, Tags=[{Key: <base>-tier, Value: data}].
   d. ListSecrets with Filters=[{Key: name, Values: [<base>]}] and confirm all three are listed.
   e. For each case, add a second filter to that same name filter and compare the returned Name set with AWS's documented answer: {description: Payment} / {description: payment} / {description: gateway} / {description: cred} -> [<base>-app-1]; {description: DATABASE} -> [<base>-app-2]; {all: gateway} / {all: GATEWAY} / {all: web} -> [<base>-app-1]; {all: <base>-tier} -> [<base>-app-1, <base>-db-1]; {all: data} -> [<base>-app-2, <base>-db-1]; {all: db} -> [<base>-db-1].
   f. ListSecrets with Filters=[{Key: name, Values: [<base>]}, {Key: all, Values: ["payment login"]}] -> expect an empty SecretList (the words are ANDed).
   g. ListSecrets with Filters=[{Key: description, Values: [Payment]}, {Key: name, Values: [<BASE-UPPERCASED>]}] -> expect an empty SecretList (`name` is case-sensitive).
   Each of d-g must be answered by one paginated ListSecrets walk, with no NextToken left outstanding on the final page.
