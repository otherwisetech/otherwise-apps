# Releases: how the Hub finds them, and how to publish one

Every Otherwise Technologies app publishes its versions as **GitHub releases**, builds only.
Open-source apps release from their own repository. Closed-source apps release from a **public,
release-only repository** named **`otherwisetech/<app>-releases`**, one per app (README, user docs,
CHANGELOG, licence, issue templates and the release files; never the source), because releases in
a private repository can't be seen or downloaded by users. The first one is
[`otherwisetech/expense-me-releases`](https://github.com/otherwisetech/expense-me-releases).
The full policy, with the list of apps, lives in otherwisetech/suite `docs/politica-releases.md`
(private).

## How Otherwise Hub discovers and installs a release

1. **Catalog**: `https://otherwisetech.com/apps.v2.json` (`catalog::MANIFEST_URL`, schema 2), else
   `https://otherwisetech.com/apps.json` (`LEGACY_MANIFEST_URL`, schema 1), else the last catalog
   saved after a successful check, else the copy compiled in (`crates/core/data/catalog.json`).
   The website builds both from its published apps and each one's latest release.
   **Why two files**: Hubs up to 0.7.x read only `apps.json` and reject it *as a whole* when one
   entry fails their validation (they then fall back to their saved catalog: they keep working and
   still update themselves through GitHub, but stop seeing catalog changes). So `apps.json` lists
   only entries every Hub accepts, and anything that needs a newer Hub goes in `apps.v2.json`
   only, with `"minHub": "<version>"`. From 0.8.0 the Hub validates **each entry on its own**
   (`catalog::parse_manifest_lenient`): a bad entry is dropped and reported in the check's errors,
   the others stay, and an entry whose `minHub` is newer than the Hub is skipped quietly. Future
   additions therefore need only a `minHub`, not a new file. Unreadable JSON or a newer `schema`
   still fail the whole file.
2. **Fresher release data**: for every catalog entry the Hub asks
   `https://api.github.com/repos/<repo>/releases/latest` **without authentication**
   (`net::fetch_latest_release`) and keeps it when it is at least as new (`catalog::apply_release`).
   Drafts, pre-releases and non-semver tags are ignored; **404 means "no release yet"**. A 403/429
   (anonymous quota: 60 requests per hour per IP) stops the round for the remaining repos.
3. **Validation** (`AppEntry::validate`): `repo` must be `otherwisetech/<name>`, the release URL must
   be under `https://github.com/<repo>/releases/` and every asset under
   `https://github.com/<repo>/releases/download/`. Any repository in the org passes, so a release-only
   repository needs no code change: its catalog entry just names it in `repo`
   (`"repo": "otherwisetech/<app>-releases"`, with an `id` that need not match; pinned by the
   `release_only_repo` test in `crates/core/tests/catalog.rs`). The only repositories outside the
   org that pass are listed exactly in `catalog::EXTRA_REPOS`: `owsas/vcity-desk-releases` (Vcity
   Desk; Vcity is owned by Otherwise but its repositories live in the `owsas` org), pinned by the
   `allowlisted_repo_outside_org` test. Hubs ≤ 0.7.x reject a whole catalog naming any other
   repository, so **such an entry goes only in `apps.v2.json`, with a `minHub`** (Vcity Desk:
   `"minHub": "0.8.0"`); 0.8.0+ drop just that entry.
4. **Asset choice** (`platform::select_asset`): the entry's `platforms.<os>[].pattern` (glob, `*`
   matches any run, e.g. `app-*-macos-arm64.zip`) with its `arch`: native first (`arm64` on Apple
   silicon), then `universal` on macOS, then `x86_64` under emulation (not on Linux). `.zip` and
   `.tar.gz` are unpacked. An app with no file for this OS / CPU (Vcity Desk and expense me ship
   only `macos` `arm64`) is listed with "not available on <OS>" and no install button
   (`Status::Unsupported`; test `mac_arm64_only_app_elsewhere_is_unsupported`).
5. **Verification** (`pipeline::fresh_digest`, `verify_file`): right before downloading, the Hub asks
   the API again for the asset's `digest`; **no SHA-256 digest, or a mismatch, means nothing is
   installed**. GitHub computes that digest for every uploaded asset; the `.sha256` files are for
   people.
6. The Hub updates itself the same way (entry `otherwise-apps`).

What this means for releases in a **private** repository: the API answers 404 to anonymous
callers, so the Hub sees "no release", and the `releases/download/…` and
`releases/latest/download/…` links are 404 for anyone not signed in to GitHub with access. A
website build with a token would only publish links that fail for everyone else. Private-repo
releases are therefore internal only.

## Publishing a release (local build)

```bash
# in the source repo: version bumped, merged to main, tests + clippy green, then
git tag vX.Y.Z && git push origin vX.Y.Z        # never move or delete tags

# build (script/bundle-macos.sh) with local paths stripped from the binary; the last matching
# prefix wins, so the more specific one goes last (add --remap-path-prefix=$PWD=<app> if the
# checkout is outside $HOME)
RUSTFLAGS="--remap-path-prefix=$HOME=~ --remap-path-prefix=$HOME/.cargo=cargo" script/bundle-macos.sh
strings "<App>.app/Contents/MacOS/<bin>" | grep /Users/    # must print nothing
ditto -c -k --keepParent "<App>.app" dist/<bin>-macos-arm64.zip
(cd dist && shasum -a 256 <bin>-macos-arm64.zip > <bin>-macos-arm64.zip.sha256)

# open source: the app's own repo; closed source: otherwisetech/<app>-releases
gh release create vX.Y.Z --repo otherwisetech/<repo> --title "<App> X.Y.Z" --notes-file notes.md \
  dist/<bin>-macos-arm64.zip dist/<bin>-macos-arm64.zip.sha256
# a release-only repo has no source to tag: add --target main

gh release view vX.Y.Z --repo otherwisetech/<repo> --json assets -q '.assets[]|.name+" "+.digest'
```

**The other platforms come from Actions** (open-source repos only: public repos get free
runner minutes). Pushing the tag starts each repo's `.github/workflows/release.yml`, which builds
`<bin>-macos-universal.zip`, `<bin>-windows-x86_64.zip` and `<bin>-linux-x86_64.tar.gz`, each
with a `.sha256`, and attaches them to the tag's release: it creates the release if the local
`gh release create` hasn't yet, otherwise it only adds (or replaces) its own files, so the local
notes and `<bin>-macos-arm64.zip` stay. If the local release comes second, add the local files
with `gh release upload` (and the notes with `gh release edit --notes-file`) instead. To rebuild
an existing tag (a failed run, a release made while Actions was down):
`gh workflow run release.yml -R otherwisetech/<repo> -f tag=vX.Y.Z` (add `-f skip_tests=true`
only when that tag's tests can't pass on a runner for a reason fixed later on main). Write the
notes for all the files; don't say a platform is missing before the run has finished.

- **Stable file names** without the version (`<bin>-<os>-<arch>.<ext>`), so
  `releases/latest/download/<file>` keeps working and the catalog patterns stay simple. The website's
  `lib/catalog.ts` assumes `<bin>-macos-universal.zip`, `<bin>-windows-x86_64.zip` and
  `<bin>-linux-x86_64.tar.gz` for open-source apps; a closed-source product lists its own files
  (e.g. only `<bin>-macos-arm64.zip`) next to its `releasesRepo` in `lib/site.ts`.
- The Hub's releases attach every file twice, as `otherwise-hub-*` and `otherwise-apps-*` (see
  CLAUDE.md).
- **Notes are for users**: what changed, in plain words. For a closed-source app never paste the
  source repo's auto-generated "What's Changed" (private PR titles and links), local paths, customer
  names or anyone's data. Keep the same text in the release-only repo's `CHANGELOG.md`.
- Check the binary before uploading: `strings <binary> | grep -i "/Users/"` finds unstripped paths;
  no API keys or internal URLs may be compiled in.

## Issues

Each public repository (the app's own, or its release-only one) takes bug reports and feature
requests through issue forms (`.github/ISSUE_TEMPLATE/bug.yml`, `feature.yml`, `config.yml` with
blank issues off and a private contact for security reports). The forms ask for the app version,
OS and chip, steps and expected result; screenshots are optional and the form asks people to cover
names, amounts, documents and paths first; a required checkbox confirms there is no personal,
financial or customer data. Never ask for a user's data file.

## Actions

Public repositories get free GitHub-hosted runner minutes. A release-only repository has no source,
so it can't build the app there; it could still run checks that need no code (assets have a
`.sha256` and a matching digest).
