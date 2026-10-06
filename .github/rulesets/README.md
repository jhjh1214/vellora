# Repository rulesets

Source of truth for the GitHub rulesets applied to this repository. Keep them in sync with the live settings. Change them via PR, then apply:

```sh
# create
gh api -X POST repos/{owner}/{repo}/rulesets --input .github/rulesets/main-branch.json
# update (find the id with: gh api repos/{owner}/{repo}/rulesets)
gh api -X PUT repos/{owner}/{repo}/rulesets/<id> --input .github/rulesets/main-branch.json
```

## Protect main (`main-branch.json`)

| Rule | Why |
|---|---|
| No deletion, no force-push | History on `main` is permanent |
| Linear history, **squash-merge only** | One reviewed, revertible commit per PR. The PR title becomes the Conventional Commit subject. |
| Pull request required | No direct pushes, not even by maintainers |
| 0 required approvals (for now) | A solo maintainer can't approve their own PR. Raise this to 1 when a second maintainer joins (GOVERNANCE.md). |
| Resolve all review threads | Review comments can't be silently ignored |
| Required checks, branch must be up to date | rustfmt, clippy + tests on all three OSes, cargo-deny, DCO sign-off |
| No bypass actors | Rules apply to everyone; changing them is a visible settings change |

When CI jobs are renamed or added, update `required_status_checks` here and re-apply the ruleset. Otherwise PRs block on checks that never report.

## Protect release tags (`release-tags.json`)

`v*` tags can't be deleted, moved or force-updated once published.
