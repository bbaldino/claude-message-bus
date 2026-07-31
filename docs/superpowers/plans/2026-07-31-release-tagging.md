# claude-message-bus Release Tagging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put `claude-message-bus` on the standard release-tagging practice. First greenfield adoption — the repo has never had CI.

**Architecture:** The git tag is the source of truth. `publish-image.yml` reacts to tag pushes and computes image tags. release-plz watches conventional commits on `main`, opens a release PR bumping `Cargo.toml`, auto-merges it, and creates the tag — which triggers the image build.

**Tech Stack:** GitHub Actions, `docker/metadata-action@v5`, `docker/build-push-action@v6`, GHCR, release-plz, Rust.

**Repo:** `github.com/bbaldino/claude-message-bus` (public) -> `ghcr.io/bbaldino/claude-message-bus`
**Standard:** `release-tagging-SKILL.md` in this room (read it first)

## Global Constraints

- `latest` means the newest release, never the tip of main.
- Never add a `paths:` filter to `publish-image.yml`.
- Never add a `type=raw,value=latest,enable={{is_default_branch}}` rule.
- `provenance: false` and `sbom: false`.
- `platforms: linux/amd64`.
- `git_tag_name = "v{{version}}"` must be bare — see the warning below.
- After Task 2, never hand-edit `Cargo.toml`'s version.

## Why this repo is the easy one

Checked against your actual code before this plan was written:

- **No `build.rs`, no `rust-embed`.** The `cargo package --verify` failure that
  cost the pilot most of its time cannot happen here.
- **`include_str!("../../schema.sql")` is safe.** `git_only` runs
  `cargo package --verify`, which builds from a packaged copy — but `schema.sql`
  is git-tracked and confirmed present in `cargo package --list`, so the packaged
  build finds it. Verified, not assumed.
- **Your commits are already conventional** (`feat:`, `fix:`, `docs:`). No commit
  style migration, unlike caas.
- **Nothing deploys this image.** The bus runs as a process on hardac, not a
  container, and no Unraid container pulls it. The `latest`-flip hazard that
  every other repo had to plan around does not apply.
- **Greenfield CI.** No existing workflow to replace or preserve — the repo has
  no `.github/` at all.

## The one sharp edge: the tag name

Your package is named `claude-bus`, the repo is `claude-message-bus`, and
`Cargo.toml` has a real `[workspace]` section. release-plz's default tag for a
workspace member is **package-name-prefixed** — `claude-bus-v0.1.0`.

That would match neither `publish-image.yml`'s `tags: ['v*.*.*']` filter nor the
baseline release-plz looks for. The tag would be created, no image would build,
and nothing would report an error.

`git_tag_name = "v{{version}}"` in Task 2 is what prevents this. Task 3 Step 5
verifies the tag is actually bare. Do not skip that check — of all the repos in
this rollout, yours is the one where this specifically bites.

## Prerequisites (human, already in progress)

1. `gh repo create bbaldino/claude-message-bus --public --source=. --push`
2. `claude-message-bus` added to the `RELEASE_BOT_TOKEN` PAT's repository list.
3. hub sets the `RELEASE_BOT_TOKEN` secret on the repo.

**Do not start Task 1 until `gh secret list` shows `RELEASE_BOT_TOKEN`.**

---

### Task 1: Add the publish workflow

**Files:**
- Create: `.github/workflows/publish-image.yml`

Nothing to delete — this is the repo's first workflow.

**Interfaces:**
- Consumes: nothing.
- Produces: `publish-image` emitting `sha-<short>` on main pushes and `X.Y.Z` / `X.Y` / `latest` / `sha-<short>` on `v*.*.*` tags.

- [ ] **Step 1: Confirm prerequisites landed**

```bash
git remote -v
gh secret list | grep RELEASE_BOT_TOKEN
```

Expected: an `origin` pointing at `bbaldino/claude-message-bus`, and a
`RELEASE_BOT_TOKEN` row. **If either is missing, stop and tell hub.**

- [ ] **Step 2: Create the workflow**

Create `.github/workflows/publish-image.yml`:

```yaml
name: publish-image

on:
  push:
    branches: [main]
    tags: ['v*.*.*']
  workflow_dispatch:

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --locked

  publish:
    needs: test
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
          # No raw latest rule: flavor latest=auto already tags latest on
          # non-prerelease semver tags. Having both makes latest bounce.
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

If `cargo test --locked` is too slow or needs extra setup for your suite, adjust
the `test` job to match how you actually run tests — but keep the job and the
`needs: test` gate.

- [ ] **Step 3: Verify it parses**

```bash
python3 -c "import yaml; w=yaml.safe_load(open('.github/workflows/publish-image.yml')); assert w['jobs']['publish']['needs']=='test'; print('ok')"
```

Expected: `ok`

- [ ] **Step 4: Commit and push**

```bash
git add .github/workflows/publish-image.yml
git commit -m "ci: publish container image to GHCR on push and release tags"
git push origin main
gh run watch --exit-status
```

- [ ] **Step 5: Verify the first image published**

```bash
SHA=$(git rev-parse --short HEAD)
docker buildx imagetools inspect ghcr.io/bbaldino/claude-message-bus:sha-$SHA
```

Expected: a single `linux/amd64` manifest, **no** `unknown/unknown` attestation
entry, and **no** `latest` tag yet — `latest` only appears once a release is cut
in Task 3. The package is public, so this works without auth.

---

### Task 2: Add release-plz

**Files:**
- Create: `release-plz.toml`
- Create: `.github/workflows/release-plz.yml`

**Interfaces:**
- Consumes: nothing yet (Task 3 provides the baseline tag).
- Produces: a workflow that opens, auto-merges, and tags releases.

- [ ] **Step 1: Create the config**

Create `release-plz.toml`:

```toml
[workspace]
# Application shipped as a container image, not a library. Never publish.
publish = false

# REQUIRED alongside publish = false. release-plz normally derives the previous
# version from crates.io; a crate never published there has nothing to compare
# against, so every run is treated as the first release and the version never
# advances — silently. This makes git tags the baseline instead.
git_only = true

# Cut a GitHub release alongside the tag so the changelog is visible there.
git_release_enable = true

# MUST be bare. This package is `claude-bus` inside a [workspace], so the
# default tag would be `claude-bus-v0.1.0` — which matches neither
# publish-image.yml's `tags: ['v*.*.*']` filter nor the baseline release-plz
# looks for. Tag created, no image built, no error reported.
git_tag_name = "v{{version}}"
```

Do not add `semver_check` or `publish_no_verify`; both were tried during the
pilot and neither helps.

- [ ] **Step 2: Create the workflow**

Create `.github/workflows/release-plz.yml`:

```yaml
name: release-plz

on:
  push:
    branches: [main]

permissions:
  contents: write
  pull-requests: write

# Serialize runs so a post-merge run cannot race the run that spawned it.
# cancel-in-progress MUST be false — cancelling mid-release can interrupt tag
# creation.
concurrency:
  group: release-plz-${{ github.ref }}
  cancel-in-progress: false

jobs:
  release-plz:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0            # release-plz needs full history
          persist-credentials: false

      - uses: dtolnay/rust-toolchain@stable

      # `release` runs FIRST — this ordering is load-bearing, see below.
      # @v0.5, NOT @v0 — there is no moving v0 tag; `@v0` hard-fails the run.
      - uses: release-plz/action@v0.5
        with:
          command: release
        env:
          # NOT secrets.GITHUB_TOKEN: tags pushed with it do not trigger
          # publish-image.yml, so the release would silently produce no image.
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
          # Must be RELEASE_BOT_TOKEN: a merge by GITHUB_TOKEN does not trigger
          # the follow-up run that cuts the tag.
          GH_TOKEN: ${{ secrets.RELEASE_BOT_TOKEN }}
        run: gh pr merge --squash --delete-branch "${{ fromJSON(steps.release_pr.outputs.pr).number }}"
```

**Why `release` comes before `release-pr`.** caas ran it the other way round and
every release after the first produced a duplicate PR and a failed run: the
post-merge run's `release-pr` step saw main bumped but not yet tagged, concluded
there was something to release, and opened a redundant PR whose auto-merge then
failed with "the merge commit cannot be cleanly created". Putting `release` first
means the post-merge run tags the version before `release-pr` looks, so
`release-pr` sees a fully-released main and opens nothing. The `concurrency`
block stops the two runs racing at all. Do not reorder these.

Releases take **two workflow runs**: this run opens and merges the PR, the merge
push triggers a second run whose `release` step creates the tag. That is by
design.

- [ ] **Step 3: Verify both parse**

```bash
python3 -c "import tomllib; tomllib.load(open('release-plz.toml','rb')); print('toml ok')"
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release-plz.yml')); print('yaml ok')"
```

Expected: `toml ok` then `yaml ok`

- [ ] **Step 4: Commit and push**

```bash
git add release-plz.toml .github/workflows/release-plz.yml
git commit -m "ci: add release-plz for automated version bumps and tagging"
git push origin main
gh run watch --exit-status
```

Expected: the job succeeds. It may report no baseline — Task 3 fixes that.

---

### Task 3: Seed the baseline tag

**This must come after Task 2.** release-plz re-checks-out the seed tag as its
baseline, so the tag must sit on a commit that already contains
`release-plz.toml`. Seeding early poisons the baseline permanently — the pilot
had to force-push main to recover.

- [ ] **Step 1: Confirm preconditions**

```bash
grep '^version' Cargo.toml     # 0.1.0
git tag --list                 # empty
ls release-plz.toml            # must exist
```

**If any tag already exists, stop and tell hub.**

- [ ] **Step 2: Create and verify the annotated tag**

```bash
git tag -a v0.1.0 -m "Release 0.1.0

Baseline tag adopting the release-tagging standard. Matches the existing
Cargo.toml version; no code change accompanies it."
git cat-file -t v0.1.0
```

Expected: `tag` (annotated). A lightweight tag would print `commit`.

- [ ] **Step 3: Push and watch**

```bash
git push origin v0.1.0
gh run watch --exit-status
```

- [ ] **Step 4: Verify the four image tags on one digest**

```bash
for t in 0.1.0 0.1 latest sha-$(git rev-parse --short v0.1.0^{commit}); do
  printf "%-16s %s\n" "$t" "$(docker buildx imagetools inspect ghcr.io/bbaldino/claude-message-bus:$t --format '{{.Manifest.Digest}}')"
done
```

Expected: all four resolve to the **same digest**. Confirm the version tag is
`0.1.0` not `v0.1.0`, and that there is no bare `0`.

- [ ] **Step 5: Verify the tag name is bare — the trap check**

```bash
git tag --list
```

Expected: exactly `v0.1.0`. **If you see `claude-bus-v0.1.0`**, `git_tag_name`
did not take effect; the tag will never match the workflow filter. Fix
`release-plz.toml`, delete the bad tag locally and on origin, and re-seed.

---

### Task 4: Verify the end-to-end release path

- [ ] **Step 1: Land a conventional commit**

Make a small real change and commit it:

```bash
git commit -m "feat: <what changed>"
git push origin main
```

- [ ] **Step 2: Confirm the PR was opened and auto-merged**

```bash
gh run watch --exit-status
gh pr list --state merged --limit 3
```

Expected: a `chore: release v0.1.1` PR, already **merged** — no click needed.

**Note `0.1.1`, not `0.2.0`** — pre-1.0 Rust, `feat:` is a patch bump.

**If the PR opened but was NOT merged**, the auto-merge step failed. Check the
step log for what the `pr` output actually contained; if there is no `number`
field, fall back to `fromJSON(steps.release_pr.outputs.pr).html_url`. Report the
real shape to hub.

- [ ] **Step 3: Confirm two runs and the tag**

```bash
gh run list --workflow=release-plz.yml --limit 3
git fetch --tags && git tag --list
```

Expected: two `release-plz` runs, and tags `v0.1.0` + `v0.1.1` — both bare.

- [ ] **Step 4: Confirm the image and version agreement**

```bash
for t in 0.1.1 0.1 latest; do
  printf "%-10s %s\n" "$t" "$(docker buildx imagetools inspect ghcr.io/bbaldino/claude-message-bus:$t --format '{{.Manifest.Digest}}')"
done
grep '^version' Cargo.toml            # 0.1.1
git describe --tags --abbrev=0        # v0.1.1
```

Expected: all three tags on one digest, `latest` moved off `0.1.0`, and
`Cargo.toml` == git tag == `0.1.1`.

- [ ] **Step 5: Report to hub**

Call the `send` tool directly:

```
send(to="hub", text="<findings>", done=true)
```

Cover:
- Any step whose `Expected:` output was wrong.
- **Whether `git_tag_name` held** — you are the only repo where the prefixed-tag
  default would actually bite, so this is the real test of that guidance.
- **Whether the auto-merge step worked**, and the actual shape of release-plz's
  `pr` output. caas is testing this too; two data points settle it.
- Whether anything about greenfield adoption (no prior CI) needed steps the plan
  omitted — you are the first repo with no existing workflow, and there may be
  more of these.
- Wall clock, split hands-on vs CI wait.
