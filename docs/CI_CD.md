# Documentation delivery

A contributor can open the ZIP offline and follow the same links as a reader on GitHub. A docs version identifies an architecture snapshot. Application binaries, model downloads and update channels have separate release lifecycles.

## Worktrees

Begin from a clean clone. Fetch the shared base and create a sibling checkout. Each checkout has its own files and index. Branches, refs and objects live in the same repository.

```sh
git fetch origin
git worktree add -b architecture ../lora-architecture origin/main
git -C ../lora-architecture status -sb
git worktree list
```

If the branch already exists, omit `-b` and supply `architecture` as the last argument. If Git lists an existing checkout for it, use that directory. Start independent changes in a different branch and worktree. Merge through review. Use `git worktree remove ../lora-architecture` only after saving changes and confirming that checkout is clean.

## Local checks

From the worktree root run these commands with Python 3.10 or later. On distributions that name it `python3`, substitute that executable. Place build output outside the checkout.

```sh
python -B -m unittest discover -s scripts -p "test_*.py"
python -B scripts/docs.py check
python -B scripts/docs.py package docs-v0.0.0 --output ../lora-docs-build
```

The first command exercises invalid versions, missing required files, broken file and heading links, missing diagrams and package reproducibility after a line-ending change. The second checks the architecture inventory, all ten ADRs, the governance files and fenced diagrams in the eight designated subsystem documents. It resolves inline Markdown links and single-line reference definitions against release members. Use relative links with simple heading fragments. Put paths containing spaces in angle brackets or percent-encode the spaces. Cite remote evidence with ordinary HTTPS links.

The checks recognize fenced diagrams with a declaration and a body. Review the rendered diagram in GitHub's preview when changing its syntax. External link availability, source accuracy and benchmark claims require review. The validator performs no network requests.

Each ZIP has sorted members, fixed metadata, stored bytes and LF line endings for text. Images retain their original bytes. The builder verifies the entries and CRCs before writing `SHA256SUMS`. The archive contains the docs, referenced artwork under `docs/assets`, README, governance files and contributor tooling. Disk paths, environment files, model weights and workspace metadata are outside the release manifest.

## Pull requests

The workflow runs `Validate docs` on branch pushes and pull requests using a GitHub-hosted Ubuntu runner. It tests, builds a sample package and checks its digest with a read-only token. On a branch push, the run also exposes the preview ZIP and SHA256SUMS in its documentation artifact for three days. Extract the outer Actions download before verifying the inner ZIP. It uses no repository secrets. Fork changes run on the pull request event. A reviewer should inspect workflow or release-script changes before merging and re-run the checks after conflict resolution. Configure `Validate docs` as a required status check on the default branch when repository rules are enabled. [Workflow](../.github/workflows/docs.yml)

## Release tags

1. Review and merge the documentation into the default branch. Review the research evidence, ADRs, local link check, diagram previews and license status of included artwork before marking a snapshot for distribution.
2. Restrict creation, update and deletion of `docs-v*` tags to release maintainers using a repository ruleset. This setting must be configured on GitHub. Tags can select the workflow revision itself, so an ancestry check in YAML alone cannot authorize a release.
3. At the reviewed commit, create an annotated version tag such as `git tag -a docs-v1.0.0 -m "Architecture 1.0.0"`. Push precisely that tag, for example `git push origin refs/tags/docs-v1.0.0`. Use a new version for corrections to published snapshots.
4. CI validates the tag as `docs-vMAJOR.MINOR.PATCH`, rejects leading zeroes, checks that the commit belongs to the default branch history, then packages and stages the verified bundle. A fresh publication job downloads that run's artifact and checks the digest and current remote tag commit. It alone receives release write access. It calls GitHub CLI with `--verify-tag` and leaves the latest application release unchanged. [Release command](https://cli.github.com/manual/gh_release_create)
5. Download the ZIP and `SHA256SUMS` from the same release, and check them together. Linux users can run `sha256sum --check SHA256SUMS`. In PowerShell compare `Get-FileHash -Algorithm SHA256 <zip>` with the hex digest in `SHA256SUMS`. The digest detects damaged or mismatched assets. Trust in their origin still rests on repository access controls.

A malformed tag, moved tag, failed integrity check or commit outside the default branch history must fail before publication. Updating or deleting an existing tag does not invoke publication. An existing release is never overwritten. If an upload fails, inspect the release, tag commit and logs. Remove only an unpublished incomplete draft after maintainer review and rerun the workflow on the original tag, or issue a new version. To test end-to-end without publishing a project release, use a disposable fork with Actions enabled, merge the candidate into its default branch, push a disposable docs tag, then verify the two downloaded assets there.

## Application gates

Once a Cargo workspace exists, add formatting, Clippy, locked unit and integration builds and an offline model fixture smoke test on each release candidate OS. Collect dependency licenses and advisories for pinned Cargo crates, native libraries, JS packages and bundled binaries. Record compiler, SDK, model digest and exact backend revision for each performance job. Keep real GPU qualification on isolated, trusted hardware and require reviewed inputs for those jobs.

Installer delivery requires a separate version namespace, release manifest, SBOM, provenance, signing and platform-specific update tests. Release candidates must pass clean install, upgrade, interrupted upgrade and rollback on each qualified OS. Keep signing credentials out of ordinary PR jobs and gate publication through a protected environment. Rebuild the same revision on two isolated runners and record any remaining nondeterminism before promising bit-identical installers. Ship model weights only after a separate redistribution review.
