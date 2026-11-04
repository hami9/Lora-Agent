# Security policy

Architecture snapshots describe proposed boundaries. Report a way to bypass one, a release integrity problem or exposure of a secret even before a runnable application exists. Identify the affected commit or docs tag and the relevant trust boundary. Use synthetic inputs and remove credentials and personal data from reproductions.

## Private reports

Use the **Report a vulnerability** option on GitHub's Security tab when the repository offers it. If it is unavailable, open an issue titled **Private security contact requested** containing only a request for a private channel. Wait for the maintainer to establish it before sharing the vulnerability details. Maintainers should enable private vulnerability reporting before the first executable release. [GitHub reporting instructions](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)

The maintainer records the affected scope, reproduces the issue privately, coordinates a correction and agrees on disclosure timing with the reporter. For a compromised documentation artifact, freeze publication, compare the tag and asset checksums to the verified build, investigate repository and workflow access, then publish a corrected snapshot with its own tag and describe the affected versions.

## Support scope

Report against any published snapshot and indicate whether the current branch still contains the issue. There is no supported application release in this architecture phase. Define security maintenance windows and a supported-version table before distributing executables.
