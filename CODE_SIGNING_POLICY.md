# Code signing policy

Status: application to SignPath Foundation pending.

GitBoost is applying for the SignPath Foundation open-source code-signing program. Until onboarding is complete, Windows downloads remain unsigned unless their Authenticode signature verifies successfully.

Free code signing provided by SignPath.io, certificate by SignPath Foundation.

## Signing scope

This policy covers official Windows artifacts published from the [GitBoost repository](https://github.com/DiscoverBox/gitboost):

- the x64 NSIS installer (`.exe`);
- the x64 MSI installer (`.msi`), when published;
- the project-owned `GitBoost.exe` included in those installers.

macOS artifacts are outside this Authenticode policy. Third-party components are kept under their upstream licenses and signatures and are not signed as GitBoost-owned code.

Historical releases may be unsigned. A release must not be described as signed unless Windows reports a valid Authenticode signature for the downloaded artifact.

## Team roles

- Authors, committers, and reviewers: [DiscoverBox organization members](https://github.com/orgs/DiscoverBox/people). Trusted members may commit project-owned source and build scripts; external contributions require maintainer review before merge.
- Signing approvers: [DiscoverBox organization owners](https://github.com/orgs/DiscoverBox/people?query=role%3Aowner).

All maintainers and signing approvers must use multi-factor authentication for GitHub and SignPath. Every release signing request requires manual approval by a signing approver.

## Trusted source, build, and release process

1. The only trusted source is a version tag in the public `DiscoverBox/gitboost` repository.
2. The version-controlled [GitHub Actions release workflow](https://github.com/DiscoverBox/gitboost/blob/main/.github/workflows/release.yml) runs tests and builds Windows artifacts from that tagged commit on GitHub-hosted runners.
3. Product metadata must identify the application as `GitBoost`, and the artifact version must match the release tag and the version in `package.json`.
4. After SignPath onboarding, only artifacts produced by that workflow may be submitted through the configured SignPath signing policy. Each request must remain traceable to its commit, tag, workflow run, and artifact.
5. Signed artifacts are published on [GitHub Releases](https://github.com/DiscoverBox/gitboost/releases). Signing credentials and private keys must never be committed to the repository.

## Privacy policy

GitBoost does not contain analytics or advertising SDKs, does not create user accounts, and does not upload settings, routes, health history, diagnostic reports, or Git usage logs to DiscoverBox. Application data is stored locally on the user's device.

GitBoost makes the following network requests to provide its requested functionality:

- It retrieves the public encrypted system-node catalog from configured CDN mirrors at startup and periodically while the application is running.
- It performs limited HTTPS health probes against configured acceleration nodes. Background health checks follow the interval selected by the user and can be disabled in Settings.
- When acceleration is enabled, Git HTTPS reads for the user's configured public repositories are routed through the selected third-party acceleration node. That service can observe the user's IP address, the public repository path, and transferred content.
- When the user starts a file download, GitBoost probes the selected acceleration URL and opens it in the user's default browser.
- Project and documentation links open their public GitHub destinations in the user's default browser.

These services receive normal connection metadata according to their own privacy policies. GitBoost is intended only for public GitHub repositories and must not be used for private repositories, credentials, tokens, or other sensitive content.

## Verification and incident response

Users should download GitBoost only from the project's GitHub Releases page and verify that any release advertised as signed has a valid Authenticode signature. If the repository, build workflow, or signing account is suspected to be compromised, maintainers will stop signing and publishing affected artifacts, investigate the incident, and work with SignPath Foundation on certificate revocation when required.
