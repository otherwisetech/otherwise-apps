# otherwise-apps — for agents and contributors

Same conventions as genealogy-rs, diskmap, rama and mosaico.

## Layout

```
crates/i18n  otherwise-apps-i18n: embedded translation catalogs, t!/tn!/tv! macros, i18n guard tests
crates/core  otherwise-apps-core: catalog (+ validation), version comparison, asset selection,
             SHA-256 verification, download, unpack, install / replace / uninstall, detection,
             Linux .desktop and Windows shortcuts, running-app quit. No UI.
crates/app   `otherwise-apps` desktop app (GPUI + gpui-component). app.rs: state, checks, jobs;
             cards.rs: grid / list; dialogs.rs: modals, settings, About; jobs.rs: background work;
             hub.rs: Today page, Entities page, Settings → Hub, otherwise:// handling;
             data_root.rs: Settings → Otherwise data folder (otherwise-link `relocate`: plan,
             confirmation listing what moves, verified move / copy / use-existing, cloud status,
             registry lock and conflicted-copy warnings)
             (`otherwise-apps mcp` runs the otherwise-hub MCP server, main.rs)
tools/icon   resvg rasteriser used by script/build-icons.sh (own workspace)
```

## Rules

- **Never install anything unverified.** Every download is checked against GitHub's asset
  `digest` (SHA-256); no digest or a mismatch → the file is deleted and nothing is installed.
  Catalog URLs are validated (org repos, the app's own release downloads only).
- **Never delete user data or an installed app.** Replaced and uninstalled apps go to the Trash
  (`install::system_trash`); the only folders removed outright are our own temp unpack and
  download folders. App settings/data dirs are never touched. Uninstall only accepts paths from
  `Layout::candidates`.
- Running apps are asked to quit only after the user agrees.
- **i18n is mandatory.** No user-facing string literal in `app/src` or `core/src`; use
  `i18n::t!` / `tn!` and add the key to **every** `crates/i18n/locales/<lang>/` catalog in the same
  change. Untranslatable literals end their line with `// i18n-ok`. Catalog descriptions of the
  apps come from the manifest (en/es/fr/pt), not from the i18n catalogs.
- Tests never touch the real Trash, /Applications or the network (except `tests/live.rs`,
  `#[ignore]`d, which installs into a temp dir). Every core change ships with a test.
- `crates/core/data/catalog.json` is the bundled fallback: a snapshot of
  https://otherwisetech.com/apps.json. Refresh it when apps are added.
- Release asset names must stay `otherwise-apps-<platform>.zip|.tar.gz` and the bundle
  `Otherwise Apps.app` / `io.github.otherwisetech.otherwise-apps`: self-update depends on them.
- **The hub uses otherwise-link** (github.com/otherwisetech/otherwise-link, by tag): discovery,
  links, Today files, the entity registry and `hub.json` follow its docs/SPEC.md. The shared
  registry stays off until the user turns it on; MCP write tools stay off until the user
  enables them. Never edit the user's Claude / Codex / MCP config: Settings shows the
  `claude mcp add` command to copy. Forward other apps' links with `open_app_link` (never back
  to `otherwise://`). Merges and deletes in the registry go through a confirmation.
  Moving the data root always goes through the confirmation that lists what moves, and is
  refused while an app has its data open (its lock).
- Comments: sparse, only non-obvious why.
- GPUI pins: `Cargo.lock` pins the same zed / gpui-component revs as the other apps. Never
  `cargo update` them; building only core can prune them from the lock — restore it.

## Validate

```
cargo test -p otherwise-apps-i18n -p otherwise-apps-core -p otherwise-apps-app
cargo clippy --workspace --all-targets
```
