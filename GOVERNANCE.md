# Governance

This document says who decides what happens to Ferry, how decisions are made and how you can take
part. It describes the project as it is today, a small project with one maintainer, and will change
when that changes.

## Maintainer

Ferry is maintained by its author, [VP859](https://github.com/VP859) on GitHub (the *maintainer*).
The maintainer:

- decides what goes into Ferry, reviews and merges pull requests and publishes releases;
- holds the release signing keys and the administrative rights to the repository and to any
  account the project has (see [FUNDING.md](docs/FUNDING.md));
- is responsible for everything Ferry contains, whoever or whatever wrote it;
- enforces the [Code of Conduct](CODE_OF_CONDUCT.md) and handles security reports
  ([SECURITY.md](SECURITY.md)).

No company, employer, sponsor or tool vendor has any say in the project.

## How decisions are made

- **Everyday changes** (bug fixes, small features, dependencies) are decided in the pull request.
- **Larger changes** (new features, protocol changes, new dependencies with network access, changes
  to the security model) start as an issue, so that anyone can comment before work begins. The
  maintainer decides and writes down the reason in the issue.
- **Protocol changes** follow [docs/PROTOCOL.md](docs/PROTOCOL.md#changing-the-protocol): an
  incompatible change gets a new protocol version and the previous one stays supported.
- **Priorities** are published in the [roadmap](ROADMAP.md). Anyone can propose an item in an issue.
- **Disagreements** are discussed in the issue. If no agreement is reached, the maintainer decides;
  you are always free to fork Ferry under its license.

## Contributions and ownership

- Contributions are licensed to everyone under the same terms as Ferry, **MIT or Apache-2.0** at
  the user's option ([CONTRIBUTING.md](CONTRIBUTING.md#license)). There is no contributor license
  agreement and no transfer of rights: you keep the copyright of what you write.
- Contributors are credited in the git history and, for notable contributions, in the
  [changelog](CHANGELOG.md).

## AI-assisted development

Most of Ferry's code, tests and documentation were written by an AI coding assistant (Claude, made
by Anthropic), following the maintainer's instructions; the README [says so](README.md#how-ferry-is-made).

- The assistant is a **tool**, like a compiler or an editor. It is not a contributor, maintainer
  or owner, and its vendor has no role in the project: no rights to it, no say in its decisions and
  no part in its governance, beyond the terms of service of the tool itself.
- The maintainer chooses what the assistant works on, reviews and tests the results and decides
  what is published. Responsibility for the result is the maintainer's.
- Contributors may use AI tools too. Say so in the pull request, check that the result is correct
  and that you have the right to contribute it, and be ready to explain it.

## Releases

Releases are made by the maintainer from tagged commits by the [release workflow](.github/workflows/release.yml),
with checksums of every file. Changes are listed in the [changelog](CHANGELOG.md).

## If the maintainer is gone

Ferry depends on one person today. If the maintainer does not respond to issues, pull requests or
security reports for six months, the project should be considered unmaintained: please fork it
rather than rely on it. The maintainer intends to add further maintainers once there are regular
contributors, chosen from people who have contributed for a while and agreed to this document; this
section will then describe how maintainers are added and removed.

## Changing this document

Changes to this document are made by pull request and stay open for comments for at least two
weeks before they are merged.
