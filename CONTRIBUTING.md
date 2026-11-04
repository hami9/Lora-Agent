# Contributing

Start from the [architecture](docs/ARCHITECTURE.md) and [roadmap](docs/DEVELOPMENT_ROADMAP.md). Pick one change with a reviewable outcome. Open an issue for changes to trust boundaries, public contracts, supported hardware or release scope before investing in an implementation.

## Change a design

Use a separate branch and worktree. Cite upstream technical documentation for existing capabilities. Record the exact revision and test environment behind a measured claim. Identify the experiment and acceptance threshold for a hypothesis. When revising a decision, preserve its history and link the replacement ADR. Distinguish a proposed feature from a tested release guarantee.

Run the local checks, preview changed diagrams and include the results and remaining gaps in your pull request. Reviewers should trace an affected user journey through the contracts, failure handling and acceptance tests. Policy and security changes require an independent review of what an untrusted input can cause. [Worktrees and checks](docs/CI_CD.md)

## Community

Keep examples synthetic and strip user paths, tokens, private prompts and model weights from issue attachments. Follow the [code of conduct](CODE_OF_CONDUCT.md) and report sensitive issues through the [security policy](SECURITY.md).
