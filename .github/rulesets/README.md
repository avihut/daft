# Repository rulesets

The two JSON files here are the rulesets applied to `avihut/daft`, in the shape
the GitHub REST API accepts. GitHub enforces the live copy; these files are the
reviewable intent and the thing you apply from, so a policy change is a pull
request like any other and `scripts/check-rulesets.sh` can say whether the live
rulesets still match what the repository says they should be.

| File                | Ruleset        | What it does                                                                                                                                                                                                                                                                                                                                                                                  |
| ------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `master.json`       | `master`       | No deletion, no force-push, linear history. Changes land only through a pull request, only by squash, only once `ci-gate` (the one required status check, from `.github/workflows/test.yml`) is green on a branch up to date with `master`, with every review thread resolved. Repository admins may bypass (that is what keeps `daft merge`'s fast-forward push working for the maintainer). |
| `release-tags.json` | `release tags` | `v*` tags can be created, moved, or deleted only by repository admins and the Wheatley release App (`release-flow.yml` pushes the tag that triggers `release.yml`). A stray tag push is a release build.                                                                                                                                                                                      |

Why zero required approvals: daft has one maintainer. A required review would
mean bypassing the rule on every own PR and a bot rubber-stamping Dependabot's —
a rule that is always bypassed protects nothing and hides that it protects
nothing. The gate that does the work is `ci-gate`; turn approvals on the day a
second maintainer can give them.

## Applying

```sh
# Update an existing ruleset (look the id up by name; ids are stable but
# repository-specific):
id=$(gh api repos/avihut/daft/rulesets --jq '.[] | select(.name == "master") | .id')
gh api -X PUT "repos/avihut/daft/rulesets/$id" --input .github/rulesets/master.json

# Create one that does not exist yet:
gh api -X POST repos/avihut/daft/rulesets --input .github/rulesets/release-tags.json

# Verify the live rulesets match these files:
scripts/check-rulesets.sh            # or: mise run validate:rulesets
```

`integration_id: 15368` is GitHub Actions (the app that reports `ci-gate`);
`actor_id: 5` is the Repository admin role; `actor_id: 2607344` is the Wheatley
GitHub App (`wheatley-the-moronic-ci-bot`), the release bot whose token
`release-flow.yml` and `mise-tool-updates.yml` use.
