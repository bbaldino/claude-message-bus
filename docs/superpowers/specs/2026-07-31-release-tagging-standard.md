---
name: release-tagging
description: Use when a project builds a Dockerfile and publishes to a container registry - setting up CI to push images, adding a publish workflow, wiring up GHCR, cutting a release, or deciding how images and git commits get versioned. Defines the standard tag scheme (annotated vX.Y.Z git tags -> X.Y.Z / X.Y / latest / sha-<short> image tags) and the conventional-commit release bots that drive it.
---

# Release tagging

Images identified only by `latest` cannot be rolled back and cannot be traced to
a commit. This standard makes the annotated git tag the single source of truth
and derives everything else from it.

Apply this whenever a project gains a Dockerfile that gets published, or when
setting up its CI.

## The contract

**Git:** a tag `vX.Y.Z` on the release commit, pushed to origin. Created by the
release bot when a release PR merges — never by hand, except a one-time seed tag
when adopting this in an existing repo.

**Tag object type varies by bot, and that is fine.** release-plz creates
*annotated* tags. release-please creates *lightweight* ones, because it tags via
the GitHub Releases API, which has no annotation option and no simple switch to
change it. Both trigger `tags: ['v*.*.*']` and both work with
`metadata-action`'s semver rules — verified on a real release. So do not assert
`git cat-file -t <tag>` returns `tag`: it returns `commit` on every
release-please repo. Hand-cut seed tags should still be annotated, since they
carry a message explaining the baseline.

**Docker** (`ghcr.io/<owner>/<name>`):

| Event | Tags emitted |
|---|---|
| push to `main` | `sha-<short>` |
| tag `vX.Y.Z` | `X.Y.Z`, `X.Y`, `latest`, `sha-<short>` |

Worked example:

```
annotated tag v0.1.0 -> commit 6acd274 (== origin/main HEAD)
image tags:             0.1.0, 0.1, latest, sha-6acd274
```

**What deployments should pin: the `X.Y` tag.** It moves with patch releases, so
a redeploy picks up fixes without an edit, while a minor bump stays a deliberate
decision. `X.Y.Z` is for reproducing an exact build; `latest` is for humans
trying something out, not for anything you depend on.

Note the moving tag does not *push* anything — it only changes what the next
pull resolves to. Something still has to trigger that pull.

Note also that `X.Y` means different things pre-1.0 depending on the bot, because
of how each ecosystem versions: on a release-please (Node) repo `feat:` is a
minor bump, so `0.2` gets bug fixes only; on a release-plz (Rust) repo `feat:` is
a patch bump, so `0.1` gets fixes *and* new features. Same tag shape, looser pin
on the Rust side.

Rules:

- **`latest` means the newest release, never the tip of main.** A pull of
  `latest` always yields a deliberately cut version.
- **The `v` prefix lives in git only.** metadata-action strips it, so image tags
  are bare (`0.1.0`, not `v0.1.0`).
- **No bare-major tag below 1.0.** metadata-action suppresses `{{major}}` while
  major is 0. Add `type=semver,pattern={{major}}` once the project crosses 1.0.
- **Pushes to main still publish**, as `sha-<short>` only. Unreleased builds stay
  pinnable without polluting `latest`.

## Canonical workflow

`.github/workflows/publish-image.yml`. Copy verbatim; the only thing that varies
is `context:`/`file:` when the Dockerfile is not at the repo root.

```yaml
name: publish-image

on:
  push:
    branches: [main]
    tags: ['v*.*.*']
  workflow_dispatch:

# main-push and tag-push are different refs and can run concurrently.
# That is fine: they write disjoint tag sets.
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  publish:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write
    steps:
      - uses: actions/checkout@v4

      - uses: docker/setup-buildx-action@v3

      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - id: meta
        uses: docker/metadata-action@v5
        with:
          images: ghcr.io/${{ github.repository }}
          # No `type=raw,value=latest,enable={{is_default_branch}}` rule here.
          # metadata-action's default `flavor: latest=auto` already applies
          # latest to non-prerelease semver tags. Adding the raw rule too makes
          # latest land on BOTH paths, so it bounces between "tip of main" and
          # "newest release" depending on which workflow ran last.
          tags: |
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=sha,prefix=sha-

      - uses: docker/build-push-action@v6
        with:
          context: .
          platforms: linux/amd64
          push: true
          provenance: false
          sbom: false
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

## Two things that must never be added

**Never add a `paths:` filter to this workflow.** GitHub ANDs `paths` with the
tag filter, so a release tag whose commit did not touch the filtered path is
*silently skipped* — no failure, no image, no signal. If path filtering is
genuinely needed, split into two workflows.

**Never add a raw `latest` rule.** See the comment in the YAML above. This is
the single easiest way to break the standard, and it fails quietly.

## Fixed choices

- `provenance: false, sbom: false` — build-push-action@v6 emits provenance
  attestations by default, adding an `unknown/unknown` entry beside the image in
  the registry. Not worth the noise for private images.
- `platforms: linux/amd64` — single-arch unless something actually needs arm64.
- `GITHUB_TOKEN` for auth, no PAT. The repo's Actions -> Workflow permissions
  must not be read-only, or the push 403s.
- Registry packages are private on first publish. The
  `org.opencontainers.image.source` label (added by metadata-action) links the
  package back to the repo.

## Release automation

Conventional commits are required. Commits accumulate on main; the bot opens a
release PR with the version bump and changelog; merging it cuts the annotated
tag; the tag push triggers the workflow above.

| Project type | Bot | Config |
|---|---|---|
| Rust | release-plz | `publish = false` unless it is a real crates.io library |
| Node | release-please | manifest form, `node` release-type |
| No manifest (pure Dockerfile) | release-please | manifest form, `simple` release-type + `version.txt` |

**The release bot must not use `GITHUB_TOKEN`.** A tag pushed with the default
`GITHUB_TOKEN` does *not* trigger other workflows — GitHub suppresses that to
prevent recursive runs. So the bot creates the tag, `publish-image.yml` never
fires, no image is built, and nothing reports an error. It fails completely
silently.

Give the bot a fine-grained PAT instead (contents: read/write, pull requests:
read/write), stored as the repo secret `RELEASE_BOT_TOKEN`. Use that one secret
name regardless of which bot the repo runs, so the convention holds across a
mixed fleet.

release-plz takes it as the action's `GITHUB_TOKEN` env var:

```yaml
      # @v0.5, NOT @v0 — release-plz/action publishes no moving v0 tag.
      # `@v0` fails the run outright with "unable to find version v0".
      - uses: release-plz/action@v0.5
        with:
          command: release
        env:
          # NOT secrets.GITHUB_TOKEN — see above.
          GITHUB_TOKEN: ${{ secrets.RELEASE_BOT_TOKEN }}
```

release-please takes it as a `token` input:

```yaml
      - uses: googleapis/release-please-action@v4
        with:
          # NOT the default GITHUB_TOKEN — see above.
          token: ${{ secrets.RELEASE_BOT_TOKEN }}
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json
```

A GitHub App token works too. The symptom to recognize: a release tag exists and
the GitHub release is published, but no `publish-image` run appears for that ref.

**Auto-merge the release PR.** Neither bot can skip the PR — for release-please
it is the mechanism, since the PR carries the version-bump commit that the tag is
cut from (`skip-github-pull-request` only splits the phases across workflows, it
does not enable direct releases). So instead of removing the PR, merge it
immediately. Add this as the step right after the bot, in the same job. Both
actions expose `prs_created` and a `pr` JSON output, so the step is identical for
either:

```yaml
      - name: Auto-merge the release PR
        if: steps.release.outputs.prs_created == 'true'
        env:
          # Must be RELEASE_BOT_TOKEN. A merge performed with GITHUB_TOKEN does
          # not trigger the follow-up run that cuts the tag — same silent
          # failure as tagging with it.
          GH_TOKEN: ${{ secrets.RELEASE_BOT_TOKEN }}
        run: gh pr merge --squash --delete-branch "${{ fromJSON(steps.release.outputs.pr).number }}"
```

Put the `id:` on whichever step *opens* the PR — for release-plz that is the
`release-pr` invocation, not the `release` one — and reference that id above.
`fromJSON(...).number` is confirmed working for release-plz.

**Order `release` BEFORE `release-pr`, and serialize the workflow.** Getting this
wrong produces a duplicate release PR and a red X on every release after the
first. Observed sequence when `release-pr` ran first:

1. A push opens a release PR, and auto-merge lands it on main.
2. That merge triggers a second run, whose `release` step correctly cuts the tag.
3. But `release-pr` in the post-merge run sees main bumped and **not yet tagged**,
   concludes there is something to release, and opens a *redundant* PR.
4. Auto-merge on that PR fails — "the merge commit cannot be cleanly created",
   since HEAD already carries that version. The run fails and the stale PR has to
   be closed by hand.

The tag, release, and images are all still correct; it is the workflow that goes
red. Two changes prevent it, both matching release-plz's own quickstart:

```yaml
# Serialize runs so a post-merge run cannot race the run that spawned it.
# cancel-in-progress MUST be false — cancelling mid-release can interrupt
# tag creation.
concurrency:
  group: release-plz-${{ github.ref }}
  cancel-in-progress: false

jobs:
  release-plz:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          persist-credentials: false

      - uses: dtolnay/rust-toolchain@stable

      # `release` FIRST. On a post-merge run it tags the just-merged version, so
      # the release-pr step below then sees a clean, fully-released main and
      # opens nothing.
      - uses: release-plz/action@v0.5
        with:
          command: release
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_BOT_TOKEN }}

      - id: release_pr
        uses: release-plz/action@v0.5
        with:
          command: release-pr
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_BOT_TOKEN }}

      - name: Auto-merge the release PR
        if: steps.release_pr.outputs.prs_created == 'true'
        env:
          GH_TOKEN: ${{ secrets.RELEASE_BOT_TOKEN }}
        run: gh pr merge --squash --delete-branch "${{ fromJSON(steps.release_pr.outputs.pr).number }}"
```

Add the same `concurrency` block to a release-please workflow. release-please
updates an existing release PR rather than opening a second one, so it does not
produce the duplicate, but serializing costs nothing and removes the race.

Sequence per push, which takes two workflow runs and is expected:

1. `feat:` lands on main -> bot opens the release PR -> this step merges it.
2. The merge pushes to main -> the workflow runs again -> the bot's `release`
   phase sees a version with no tag and creates it.
3. The tag triggers `publish-image`.

Consequences worth accepting deliberately: **every releasable commit cuts a
release**, so versions advance quickly and there is no preview of the changelog
before it publishes. That is the trade for zero-click delivery. If a repo should
batch several commits per release, leave the PR un-merged there instead.

**`publish = false` for Rust applications.** These ship as container images, not
crates. It avoids needing a `CARGO_REGISTRY_TOKEN` and removes a class of failed
releases.

**`publish = false` alone is broken — you also need `git_only = true`.** This is
not optional and it fails in the most confusing way possible. release-plz
normally derives the previous version from the *cargo registry*. For a crate that
has never been published to crates.io, there is nothing to compare against, so
every run is treated as the crate's first release: the diff is the entire
history, the computed next version equals whatever `Cargo.toml` already says, and
the version never advances. You get a release PR that changes nothing, or no PR
at all — with no error.

`git_only = true` tells release-plz to use git tags as the baseline instead,
which is what the whole standard is built on. Canonical config for a Rust app:

```toml
[workspace]
# Application shipped as a container image, not a library.
publish = false
# REQUIRED alongside publish = false: derive the previous version from git tags
# rather than crates.io. Without it, an unpublished crate never bumps.
git_only = true

git_release_enable = true

# Keep this BARE. release-plz's default for a workspace member is a
# package-name-prefixed tag (`my-crate-v0.1.0`), which matches neither
# publish-image.yml's `tags: ['v*.*.*']` filter nor the baseline release-plz
# greps for. A prefixed tag silently breaks the tag -> image trigger.
git_tag_name = "v{{version}}"
```

Those four fields are the whole config. `semver_check = false` and
`publish_no_verify` are not needed and do not help — don't add them while
debugging.

**Budget for the CI cost `git_only` adds.** Because it runs
`cargo package --verify`, *every* release-plz run compiles the crate — roughly
2-4 minutes, which a normal release-plz repo does not pay. Repos with large
dependency trees sit at the top of that range.

**`git_only = true` runs `cargo package --verify`,** which checks out the last
tag and compiles it. If a `build.rs` writes into the source tree, this fails:

```
Source directory was modified by build.rs ... Added: <path> ... pass --no-verify
```

No release-plz setting skips this — `publish_no_verify` only applies to the
publish path, which is disabled here. The fix is to make the packaged tree
already contain what `build.rs` creates, so the write becomes a no-op: commit a
tracked empty placeholder at that path (e.g. an empty `ui/dist/.gitkeep` for a
`rust-embed` setup whose build script does `create_dir_all("ui/dist")`). Check
for a source-writing build script before adopting this in any Rust repo.

**Pre-1.0 crates bump differently than you expect.** For a `0.x` version,
release-plz correctly treats `feat:` as a **patch** bump (`0.1.0` -> `0.1.1`),
not a minor one. Minor bumps only happen at `>=1.0`, or from a breaking change
while still on `0.x`. Do not write "expect 0.2.0 after a feat" into any plan for
a pre-1.0 repo.

**release-please: declare the version, don't let it be discovered.** Use the
manifest form rather than the bare `release-type:` input. release-please
otherwise infers its baseline from the latest GitHub *Release* — and a repo with
a git tag but no GitHub Release (easy to end up with when the tag was cut by
hand) has nothing to anchor on, so it misversions silently. This is the same
failure class as release-plz's `git_only` problem. State the current version
outright instead:

`.release-please-manifest.json` — for a package at the repo root use `"."`:

```json
{ "webapp": "0.1.0" }
```

`release-please-config.json`:

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "include-component-in-tag": false,
  "packages": {
    "webapp": { "release-type": "node" }
  }
}
```

`include-component-in-tag: false` is the counterpart to release-plz's
`git_tag_name` — without it the tag gets a package-name prefix that will not
match `publish-image.yml`'s `v*.*.*` filter.

**The two bots version pre-1.0 releases differently, and that is intentional.**
On `0.x`, release-plz treats `feat:` as a *patch* bump (Cargo's convention,
where the minor slot acts as the major). release-please treats it as a *minor*
bump, and a breaking change as `1.0.0`. Each is idiomatic for its ecosystem, so
neither is overridden — but do not expect a Rust repo and a Node repo to produce
the same version from the same commit. If a Node repo should not go to `1.0.0`,
avoid breaking-change footers until you mean it.

**Polyglot repos that build one image get one version.** Pick the manifest where
the substance lives (usually the backend) as the authority; mark the other
`"private": true` and leave it at `0.0.0`. Nothing consumes it separately.

Do not reach for release-please's manifest mode or independent per-package
versions here. That machinery is for monorepos publishing several packages on
separate cadences to a registry. A repo that builds one image is one deployable
with one version, and the frontend is a build artifact of it, not a released
package.

Caveat: release-plz scopes changes to a package's directory, so if the crate is
in a subdirectory, verify that a commit touching only the *other* language still
produces a release PR. If it does not, scope the package to the repo root or use
release-please instead.

**Injecting the version into the app.** Because the non-authoritative manifest
is frozen at `0.0.0`, nothing in the app may read its version from there — a UI
wired to its own `package.json` will permanently report `0.0.0`. Get the version
from the authority instead:

- Backend: read it from the language's own build constant (`CARGO_PKG_VERSION`,
  etc.) and expose it on an API endpoint or health response.
- Frontend: have it call that endpoint, or inject at build time via a
  `--build-arg` fed from the git tag.

If injecting at build time, pass the tag through the workflow explicitly — the
image tag is not visible to the build:

```yaml
      - uses: docker/build-push-action@v6
        with:
          build-args: |
            VERSION=${{ steps.meta.outputs.version }}
```

## Adopting this in an existing repo

**Order matters here.** The seed tag must be cut on a commit that *already*
contains the release tooling and any packaging fixes, because release-plz
re-checks-out that tag as its baseline. A tag cut before the fixes is a poisoned
baseline that keeps failing forever, and recovering means force-pushing main and
moving the tag. Do not seed early.

1. **Check what pulls `latest` first.** If the repo currently publishes `latest`
   from main, anything deploying it (compose files, Home Assistant configs,
   Unraid templates) will silently stop receiving main builds and pin to the last
   release. Decide whether those consumers should move to a pinned tag.
2. Replace the workflow with the canonical one above.
3. Add the release bot for the language, including `git_only = true` for a Rust
   app and any `build.rs` packaging fix it needs. **Land this before step 4.**
4. **Seed a tag**, on the commit from step 3. Both bots use the last git tag as
   their baseline. A repo with no tags needs one hand-made annotated tag matching
   its current manifest version before the bot's first run, or version history
   starts from the wrong place. Cut it in the same sitting as the workflow
   change, so `latest` is not left stale.
5. Verify: land a conventional commit, confirm the bot opens a release PR, merge
   it, confirm the tag and all four image tags appear with no manual steps.

If the repo does not use conventional commits yet, that migration has to happen
before the bot can infer version bumps. Note the seed tag makes this cheaper than
it sounds: the bots only parse commits *since the last tag*, so seeding at
current HEAD puts all pre-migration history where it is never read. The migration
is only "conventional commits from the seed tag onward" — no history rewrite.

**Expect a one-time lint cleanup on Rust repos.** The CI `test` job runs clippy
on the newest stable toolchain, which usually catches lints a locally-pinned
older toolchain was not flagging (`collapsible_match`, `len_zero`,
`useless_conversion`, `unused_imports` all showed up on one adoption). They are
mechanical, but they block the first push, so fix them in one commit before
adopting rather than being surprised mid-task.

**Verifying image tags.** `gh api /user/packages/container/<name>/versions`
needs the `read:packages` scope, which a default `gh auth login` does not have —
it 403s. Use the registry directly instead, which works anonymously on a public
package:

```bash
docker buildx imagetools inspect ghcr.io/<owner>/<name>:<tag>
```
