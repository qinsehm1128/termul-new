#!/usr/bin/env bash
#
# G5 — legacy brand residue gate.
#
# Fails (exit 1) when a legacy brand string survives anywhere outside an
# explicitly registered site. Every exemption below is a named path — or a named
# `path:line:text` triple — that a reviewer can read and check. Nothing is
# hidden by `.gitignore`, by omitting `--hidden`, or by a wildcard that happens
# to swallow a real hit.
#
# Two hard rules this file exists to enforce on itself:
#
#   1. Text mode is forced (`rg -a`). A raw NUL byte in a `.ts` file makes
#      `file(1)` classify it as `data` and makes an ordinary grep skip the whole
#      file as binary — a residual scan would then report success while missing
#      every match in it. That happened once during the rename (a memo-key
#      delimiter in `bundled-themes.ts`), so it is guarded rather than trusted.
#   2. `ALLOWED_SITE` entries pin a line NUMBER as well as the text. If the text
#      is still present but has moved, the gate reports DRIFT and fails, forcing
#      the exemption to be re-audited instead of silently following the string
#      around the file.
#
# Stages:
#   1. legacy-literal scan   — the `termul` family, all case forms
#   2. case-form census      — Termul / termul / TERMUL counted separately
#   3. reverse `SE_` scan    — delegated to the frozen env-name inventory gate
#   4. FORBID-07             — brand-seam thread-affinity gate
#
# Usage: bash scripts/check-brand-residue.sh [--no-suites]
#   --no-suites  run stages 1-2 only (no bun/cargo), for a fast local check.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

RUN_SUITES=1
[[ "${1:-}" == "--no-suites" ]] && RUN_SUITES=0

# --------------------------------------------------------------------------
# Directories and binary shapes the scan never enters. These are build output,
# vendored code, VCS internals and image blobs — not exemptions for our code.
# --------------------------------------------------------------------------
SCAN_GLOBS=(
  -g '!node_modules'
  -g '!reference'
  -g '!dist'
  -g '!dist-web'
  -g '!target'
  -g '!.git'
  -g '!.workflow'
  -g '!*.lock'
  -g '!bun.lock*'
  -g '!*.png'
  -g '!*.icns'
  -g '!*.ico'
  -g '!*.webp'
  -g '!*.jpg'
  -g '!*.jpeg'
)

# --------------------------------------------------------------------------
# WHITELIST A — whole files that are legitimate homes for a legacy literal.
#
# FORBID-04: `LEGACY_*` constants live in exactly two files, and the frozen
# fixture roots are byte-pinned pre-rename user data (FORBID-03). Nothing else
# may hold a legacy brand string by charter.
# --------------------------------------------------------------------------
ALLOWED_PATHS=(
  'src/shared/brand.ts'
  'src-tauri/src/brand.rs'
  'src/__fixtures__/legacy-brand/'
  'src-tauri/tests/fixtures/legacy-brand/'
  # This file. A scanner has to spell what it looks for; it cannot be its own
  # subject, for the same reason `env-name-parity.test.ts` excludes itself.
  'scripts/check-brand-residue.sh'
)

# --------------------------------------------------------------------------
# WHITELIST A2 — tests whose SUBJECT is the legacy contract itself.
#
# A migration harness has to spell the pre-rename value: `expect(LEGACY.themeId)
# .toBe('termul')` is the assertion, and rewriting it to read the constant it is
# checking turns the test into a tautology that passes no matter what the
# constant holds. Same for the Rust harnesses that seed a legacy keychain entry
# or a pre-rename on-disk tree and then prove production still reads it.
#
# Named one file at a time rather than by a `*brand*` glob: a glob would also
# swallow a future `brand-new-feature.test.ts` that has no business holding a
# legacy literal.
#
# FLIP TRIGGER: when a harness stops testing a legacy contract, delete its line.
# --------------------------------------------------------------------------
ALLOWED_PATHS+=(
  'src/shared/brand.test.ts'
  'src/shared/types/conversation.types.brand.test.ts'
  'src/__fixtures__/legacy-brand-manifest.test.ts'
  'src/renderer/lib/browser/terminal-url-navigation.brand.test.ts'
  'src/renderer/lib/tauri-stubs/plugin-store.brand.test.ts'
  'src/renderer/lib/themes/theme-id.brand.test.ts'
  'src/renderer/stores/acp-store.brand.test.ts'
  'src-tauri/tests/brand_migration_e2e.rs'
  'src-tauri/tests/brand_seam_thread_affinity.rs'
  'src-tauri/tests/legacy_brand_appdata_roots.rs'
  'src-tauri/tests/legacy_brand_created_by.rs'
  'src-tauri/tests/legacy_brand_fixture_manifest.rs'
  'src-tauri/tests/legacy_brand_keychain.rs'
  'src-tauri/tests/legacy_brand_skill_manifest.rs'
  'src-tauri/tests/legacy_brand_skill_marker.rs'
  'src-tauri/tests/legacy_brand_ssh_known_hosts.rs'
  'src-tauri/tests/legacy_brand_state_roots.rs'
  'src-tauri/tests/legacy_brand_worktree.rs'
)

# --------------------------------------------------------------------------
# WHITELIST B — untracked working files owned by the user, deliberately not
# renamed and deliberately not committed (R-SCOPE-UNTRACKED-DOCS). Named one by
# one rather than by wildcard so that a seventh file appearing under
# `docs/borrowing/` is a new decision rather than a silent inheritance.
#
# FLIP TRIGGER: if any of these is ever `git add`ed, delete its line here and
# rename the file under D5 (same trigger as T-D06).
# --------------------------------------------------------------------------
ALLOWED_PATHS+=(
  'config.toml'
  'docs/borrowing/README.md'
  'docs/borrowing/grok-bot-borrowing-plan.md'
  'docs/borrowing/nyaterm-borrowing-plan.md'
  'docs/borrowing/obsidian-doc-capability-feasibility.md'
  'docs/borrowing/terminal-renderer-dom-fallback.md'
  'docs/borrowing/thinkrail-borrowing-plan.md'
  'docs/leftover-glyphs-handoff.md'
)

# --------------------------------------------------------------------------
# WHITELIST C — individual registered sites, as `path:line:text`.
#
# `text` must be a substring of the matched line. Both the line number and the
# text must agree; a match on text at a different line is reported as DRIFT.
# --------------------------------------------------------------------------
ALLOWED_SITES=(
  # -- serde attributes. A serde attribute takes a string literal and cannot
  #    take a `const`, so the legacy value has to be spelled at the attribute.
  #    Each is guarded by a source-text test that asserts it equals the brand
  #    constant character for character, so the exemption cannot drift apart
  #    from `brand.rs` without a red test.
  'src-tauri/src/skills/provisioner.rs:134:#[serde(alias = "managedByTermul")]'
  'src-tauri/src/conversation/contracts.rs:314:#[serde(rename = "termul")]'

  # -- iOS compatibility reads. Six `legacy*` constants, each read-only, each
  #    naming a key or directory already on a paired phone. Swift has no
  #    cross-module brand seam, so the value is spelled at the read site.
  'ios/SeRemote/SeRemote/App/AppSettings.swift:46:legacyLanguageKey = "termul.app.language"'
  'ios/SeRemote/SeRemote/App/AppSettings.swift:47:legacyAppearanceKey = "termul.app.appearance"'
  'ios/SeRemote/SeRemote/Chat/ChatTranscriptCache.swift:127:legacyDirectoryName = "TermulRemote"'
  'ios/SeRemote/SeRemote/Models/ConnectionStore.swift:20:legacyStorageKey = "termul.remote.savedLinks"'
  'ios/SeRemote/SeRemote/Networking/KeychainStore.swift:10:legacyService = "com.termul.remote.pairing"'
  'ios/SeRemote/SeRemote/Terminal/TerminalTextScale.swift:10:legacyStorageKey = "termul.companion.terminalTextScale"'

  # -- homebrew `zap trash`. The cask must remove BOTH identifiers' data roots:
  #    a user uninstalling today may have installed before the rename. The block
  #    lists the canonical paths first and the legacy ones after, and the
  #    in-file comment explains why removing them is not a migration violation.
  'scripts/release/homebrew.sh:140:~/Library/Application Support/com.termul-manager.app'
  'scripts/release/homebrew.sh:141:~/Library/Caches/com.termul-manager.app'
  'scripts/release/homebrew.sh:142:~/Library/HTTPStorages/com.termul-manager.app'
  'scripts/release/homebrew.sh:143:~/Library/Logs/com.termul-manager.app'
  'scripts/release/homebrew.sh:144:~/Library/Preferences/com.termul-manager.app.plist'
  'scripts/release/homebrew.sh:145:~/Library/Saved Application State/com.termul-manager.app.savedState'
  'scripts/release/homebrew.sh:146:~/Library/WebKit/com.termul-manager.app'

  # -- scan/ignore globs that name the frozen legacy fixture roots and the
  #    pre-rename per-repo workspace dir. Renaming them would stop the scanner
  #    and the VCS from seeing the very trees the migration harness pins.
  'biome.json:17:"!**/.termul"'
  'biome.json:21:"!ios/TermulRemoteTests/Fixtures/legacy-brand"'
  '.gitignore:95:.termul/'
  '.gitignore:125:# `gitignore` file and `.termul/` tree are the contract under test. The'
  '.gitignore:128:!/src-tauri/tests/fixtures/legacy-brand/fake-user-repo/.termul/'

  # -- Tauri fs scope. `$HOME/**` already covers everything below home; these
  #    two entries exist to name the pre-rename roots explicitly so a reviewer
  #    can see migration reads are still permitted.
  'src-tauri/capabilities/default.json:39:{ "path": "$APPDATA/termul/**" }'
  'src-tauri/capabilities/default.json:41:{ "path": "$HOME/.termul/**" }'

  # -- migration inputs. The detector looks for the pre-rename store file, and
  #    the legacy binding hash is a domain separator over data already written;
  #    changing either makes existing user data unreachable.
  'src-tauri/src/migration_detect.rs:58:pub const PERSISTENCE_STORE_FILE: &str = "termul-data.json";'
  'src-tauri/src/conversation/migration/legacy.rs:1380:hasher.update(b"termul-legacy-binding\0");'

  # -- explicit legacy spellings carried by `brandedStorageKey`, so a flipped
  #    web-storage key can still read the value the user already has. Write
  #    paths use `.canonical`; these strings are never written again.
  'src/renderer/lib/web-tab-session.ts:1:import { brandedStorageKey, readBrandedStorage } from @/lib/brand-storage-key'
  'src/renderer/lib/parse-unified-diff.ts:1:import { brandedStorageKey, readBrandedStorage } from @/lib/brand-storage-key'
  'src/renderer/lib/companion-terminal-text-scale.ts:10:termul.companion.terminalTextScale'
  'src/renderer/components/ProjectSidebar.tsx:47:import { brandedStorageKey, readBrandedStorage } from @/lib/brand-storage-key'

  # -- the wire union has to name both discriminants: one is what is already
  #    on disk, the other is what gets written now.

  # -- prose that NAMES the pre-rename spelling in order to explain it: a
  #    migration note, a lesson recorded in a test header, a comment saying
  #    which value is legacy. Rewriting these to the new spelling would delete
  #    the very fact they exist to record.
  'src-tauri/src/remote/tunnel/frp.rs:222:/// The obvious "fix" — `assert!(toml.contains("name = \"termul\""))` — would be'
  'src-tauri/src/remote/tunnel/frp.rs:224:/// `render_frpc_toml`, so one `sed s/termul/se-manager/g` rewrites both and the'
  'scripts/mobile-host-probe.ts:351:if (/se-manager|se-server|termul/i.test(line)) named.add(port)'
  'scripts/tests/artifact-name-derivation.test.ts:16:* the loudest one — it was `termul`, matching neither upstream, and was carried'
  'scripts/tests/artifact-name-derivation.test.ts:115:// Ledger entry struck by T-B04. The cask token was `termul` — neither the'
  'scripts/tests/ios-legacy-brand-parity.test.ts:343:// settings still said `Termul`. Measured on the built product rather than'
  'scripts/tests/ios-legacy-brand-parity.test.ts:344:// inferred — `TermulRemote.app/Info.plist` carried'
  'scripts/tests/ios-legacy-brand-parity.test.ts:345:// `CFBundleDisplayName = Termul` next to localized tables reading `Se`.'
  'scripts/tests/brand-mirror-parity.test.ts:7:* `termul-plan` in Rust while TypeScript — the side that actually writes the'
  'src/renderer/components/ProjectSidebar.tsx:150:// The footer label used to hard-code "Termul v0.4.10" — both the brand and'
  'src/renderer/components/chat/ChatMessage.test.tsx:98:href="termul-file-path:src%2Frenderer%2FApp.tsx%3A42"'
  'src/renderer/components/terminal/terminal-webgl-repair.ts:150:* See .workflow/sessions/20260824-ralph-termul-leftover-glyphs/dod-amendment-01.md'

  # -- the two on-disk store files are STILL named for the old brand, and are
  #    named correctly here. Renaming them needs a file-level migration: the
  #    tree migration carries the whole appdata directory across bundle ids but
  #    keeps the file names inside it, so flipping the constant alone would
  #    orphan every persisted setting and session. Out of scope for this pass.
  'docs/development-guide.md:170:- `termul-data.json` — general app persistence'
  'docs/development-guide.md:171:- `termul-sessions.json` — session persistence'
  'docs/architecture.md:260:- verified, fail-closed legacy ACP import from `termul-data.json`'
  'src-tauri/src/migration_run.rs:222:// reconstruct their keys from `termul-data.json` and'
  'src-tauri/src/commands.rs:4433:/// Desktop MCP servers live in `termul-data.json["acp/mcp-servers"]`'
  'src/renderer/lib/tauri-session-api.ts:7:*   shaped `${STORE_FILE}::${key}` (e.g. `termul-sessions.json::sessions/auto-save`),'
  'src/renderer/lib/tauri-session-api.ts:60:* `${STORE_FILE}::${key}` (e.g. `termul-sessions.json::sessions/auto-save`),'

  # -- explicit legacy spellings carried so a flipped web-storage key can still
  #    read the value the user already has. Writes use `.canonical`.
  'src/renderer/lib/brand-storage-key.ts:7:* brand glued on by hand (`termul.gitDiffViewMode`, `termul-ssh-panel-height`),'
  'src/renderer/lib/brand-storage-key.ts:9:* only knows the `termul:` form. Writing the flipped key without carrying the'

  # -- the wire union has to name both discriminants: one is on disk already,
  #    the other is what gets written now.

  # -- the landing asset really is still `termul.svg`; the comment points at
  #    the file that exists. Renaming the asset is a landing-repo action.
  'src/renderer/components/SeMark.tsx:6:* at `landing/public/termul.svg` (white fill); this is the theme-aware variant.'

  # -- `deepLinkScheme` has not flipped (it is still `termul` in both brand
  #    tables). These two describe that state accurately.
  'ios/README.md:21:Deep link: `se://open?url=<percent-encoded-access-url>`. Encode the `#access_token` frag'
  'ios/SeRemote/SeRemote/Models/RemoteLink.swift:97:/// The pre-rename `termul` scheme is deliberately absent, not forgotten.'

  # -- a captured scan artifact holding the author's local path at capture
  #    time. Rewriting it would falsify the record.
  'docs/project-scan-report.json:9:"project_root": "E:/open-source/PecutAPP/termul",'
  'docs/project-scan-report.json:10:"project_knowledge": "E:/open-source/PecutAPP/termul/docs",'

  # -- final pass: sites whose matched text carries quotes; pinned on a
  #    quote-free substring so the entry stays readable.
  'src/shared/types/conversation.types.ts:150:termul'
  'src/shared/types/conversation.types.ts:154:termul'
  'src/renderer/components/ProjectSidebar.tsx:1827:termul-ssh-panel-height'
  'src/renderer/lib/tauri-session-api.ts:29:termul-sessions.json'
  'src/renderer/lib/tauri-persistence-api.ts:4:termul-data.json'
  'src/renderer/components/chat/chat-markdown-file-links.ts:15:termul-file-path:'
  'src/renderer/lib/parse-unified-diff.ts:214:termul.gitDiffViewMode'
  'src/renderer/lib/web-tab-session.ts:17:termul.web.focusedSessionId'
  'src/renderer/lib/__tests__/tauri-session-api.web.test.ts:7:* `termul-sessions.json::sessions/auto-save`), matching the spec'
  'src/renderer/lib/__tests__/tauri-session-api.web.test.ts:41:termul-sessions.json'

  # -- upstream fork gate. `mannnrachman/termul` is a different repository;
  #    these guards exist to keep the workflow from running in forks.
  ".github/workflows/fork-monitor.yml:25:mannnrachman/termul"
  ".github/workflows/fork-monitor.yml:53:mannnrachman/termul"
  ".github/workflows/fork-monitor.yml:96:mannnrachman/termul"
  ".github/workflows/fork-monitor.yml:122:mannnrachman/termul"
)

# --------------------------------------------------------------------------
# OUT-OF-SCOPE REGISTER — real, live, correct references to names this session
# is not renaming. Reported on every run, never silently dropped, and never
# pointed at a nonexistent address to make the gate green.
#
# Each entry is `path:pattern`; `pattern` is an extended regex matched against
# the line text. Paths are named individually.
# --------------------------------------------------------------------------
OUT_OF_SCOPE=(
  # ---- Class A: `qinsehm1128/termul-new` is the repository this project
  #      actually lives in today. Renaming a GitHub repository is out of scope
  #      for this session; every one of these URLs resolves right now and
  #      changing it would point release/update traffic at nothing.
  'package.json:qinsehm1128/termul-new'
  'README.md:qinsehm1128/termul-new'
  'CONTRIBUTING.md:(qinsehm1128/termul-new|YOUR_USERNAME/termul|cd termul$)'
  'src-tauri/tauri.conf.json:qinsehm1128/termul-new'
  'src-tauri/src/lib.rs:qinsehm1128/termul-new'
  'src-tauri/src/updater_api.rs:qinsehm1128/termul-new'
  'src-tauri/src/server_update.rs:(qinsehm1128/termul-new|termul GitHub)'
  'src/renderer/lib/tauri-updater-api.ts:qinsehm1128/termul-new'
  'src/renderer/lib/tauri-updater-api.test.ts:qinsehm1128/termul-new'
  'src/renderer/lib/__tests__/tauri-updater-api.test.ts:qinsehm1128/termul-new'
  'src/renderer/lib/__tests__/tauri-release-notes.test.ts:qinsehm1128/termul-new'
  'src/renderer/lib/tauri-release-notes.ts:qinsehm1128/termul-new'
  'scripts/install.sh:REPO="termul-new"'
  'scripts/release/homebrew.sh:qinsehm1128/termul-new'
  'scripts/release/merge-updater-manifests.mjs:qinsehm1128/termul-new'
  'scripts/release/merge-updater-manifests.test.ts:qinsehm1128/termul-new'
  'scripts/release/prepare-platform-artifacts.mjs:qinsehm1128/termul-new'
  'scripts/release/prepare-platform-artifacts.test.ts:qinsehm1128/termul-new'
  'scripts/release/prepare-server-artifacts.mjs:qinsehm1128/termul-new'
  'scripts/release/prepare-server-artifacts.test.ts:qinsehm1128/termul-new'
  'scripts/tests/install.bats:qinsehm1128/termul-new'
  '.github/workflows/publish-homebrew.yml:(qinsehm1128/termul-new|homebrew-termul-new)'
  '.github/workflows/star-history.yml:qinsehm1128/termul-new'
  '.github/ISSUE_TEMPLATE/config.yml:qinsehm1128/termul-new'

  # ---- Class B: the AUR package name is registered off-site. Renaming it is a
  #      packaging action outside this repository (vars.AUR_PACKAGE).
  '.github/workflows/publish-aur.yml:termul'
  'src/renderer/lib/tauri-updater-api.ts:yay -S termul-manager'
  'src/renderer/components/UpdateAvailableToast.tsx:yay -S termul-manager'

  # ---- Class C: UPSTREAM repository. `gnoviawan/termul` is the project this
  #      codebase derives from. It MUST STAY — it is attribution, not residue.
  'landing/src/data/contributors.ts:gnoviawan/termul'
  'landing/scripts/sync-contributors.ts:gnoviawan/termul'

  # ---- Class D: `termul.dev` — the live production domain. Domain migration is
  #      out of scope in policy.json; these are canonical URLs, sitemap entries
  #      and OG tags that must keep resolving.
  'landing/index.html:termul\.dev'
  'landing/public/robots.txt:termul\.dev'
  'landing/public/sitemap.xml:termul\.dev'
  'landing/react-ssg.config.ts:termul\.dev'
  'landing/README.md:termul\.dev'
  'landing/tests/testimonials-api.test.ts:termul\.dev'

  # ---- Class E: landing asset filenames. Renaming these needs a `git mv` of
  #      binaries plus a Cloudflare cache purge; deferred with T-B07.
  'landing/index.html:(bg-termul\.webp|termulmock\.png|termul\.svg)'
  'landing/src/components/sections/Hero.tsx:(bg-termul\.webp|termulmock\.png|termul\.svg)'
  'landing/src/components/ui/Logo.tsx:termul\.svg'
  'landing/src/data/features.ts:(bg-termul\.webp|termulmock\.png|termul\.svg)'
  'landing/src/lib/links.ts:qinsehm1128/termul-new'

  # ---- Class A (cont.): the same live repository, reached from files the
  #      register did not name yet. Each URL resolves today; repointing it
  #      would break a download, a release link or a tap lookup.
  'README.md:(gnoviawan/termul|cd termul-new)'
  'docs/deployment-guide.md:(qinsehm1128/termul-new|homebrew-termul-new)'
  'landing/index.html:qinsehm1128/termul-new'
  'landing/scripts/sync-contributors.ts:qinsehm1128/termul-new'
  'src/renderer/App.test.tsx:qinsehm1128/termul-new'
  'src/renderer/lib/__tests__/tauri-opener-api.web.test.ts:qinsehm1128/termul-new'

  # ---- Class C (cont.): upstream attribution. `gnoviawan/termul` is the
  #      project this codebase derives from; the PR numbers only exist there.
  'docs/terminal-runtime-evaluation.md:(gnoviawan/termul|qinsehm1128/termul-new|Termul (terminal performance|xterm 6\\.1 migration|minimize/restore fix) PR)'

  # ---- Class F: `@termulmanager` is an unregistered social handle. Pointing it
  #      at `@semanager` before that handle is registered would publish a dead
  #      link, so it stays until the handle exists.
  'landing/index.html:@termulmanager'
)

# --------------------------------------------------------------------------

red=0
declare -a RESIDUE=()
declare -a DRIFT=()
declare -a REGISTERED=()

is_allowed_path() {
  local path="$1" allowed
  for allowed in "${ALLOWED_PATHS[@]}"; do
    if [[ "$allowed" == */ ]]; then
      [[ "$path" == "$allowed"* ]] && return 0
    else
      [[ "$path" == "$allowed" ]] && return 0
    fi
  done
  return 1
}

# Returns 0 = registered here, 1 = not registered, 2 = text found but line moved.
check_site() {
  local path="$1" line="$2" text="$3" entry site_path site_line site_text drift=1
  for entry in "${ALLOWED_SITES[@]}"; do
    site_path="${entry%%:*}"
    local rest="${entry#*:}"
    site_line="${rest%%:*}"
    site_text="${rest#*:}"
    [[ "$site_path" == "$path" ]] || continue
    [[ "$text" == *"$site_text"* ]] || continue
    [[ "$site_line" == "$line" ]] && return 0
    drift=2
  done
  return $drift
}

is_out_of_scope() {
  local path="$1" text="$2" entry oos_path oos_pattern
  for entry in "${OUT_OF_SCOPE[@]}"; do
    oos_path="${entry%%:*}"
    oos_pattern="${entry#*:}"
    [[ "$oos_path" == "$path" ]] || continue
    [[ "$text" =~ $oos_pattern ]] && return 0
  done
  return 1
}

echo "== stage 1/4 — legacy brand literal scan (text mode forced) =="

while IFS= read -r hit; do
  path="${hit%%:*}"
  path="${path#./}"
  rest="${hit#*:}"
  line="${rest%%:*}"
  text="${rest#*:}"

  is_allowed_path "$path" && continue

  check_site "$path" "$line" "$text"
  case $? in
    0) continue ;;
    2) DRIFT+=("$path:$line: $text") ; continue ;;
  esac

  if is_out_of_scope "$path" "$text"; then
    REGISTERED+=("$path:$line: $text")
    continue
  fi

  RESIDUE+=("$path:$line: $text")
done < <(rg -a -i -n --hidden 'termul' "${SCAN_GLOBS[@]}" . 2>/dev/null)

if ((${#DRIFT[@]})); then
  echo
  echo "-- DRIFTED REGISTRATIONS (${#DRIFT[@]}) — text still present, line number moved."
  echo "   Re-audit the site and update its ALLOWED_SITES line number."
  printf '   %s\n' "${DRIFT[@]}"
  red=1
fi

if ((${#RESIDUE[@]})); then
  echo
  echo "-- UNREGISTERED LEGACY BRAND RESIDUE (${#RESIDUE[@]})"
  printf '   %s\n' "${RESIDUE[@]}"
  red=1
else
  echo "   ok — zero unregistered legacy brand strings."
fi

echo
echo "-- out_of_scope_pending register: ${#REGISTERED[@]} occurrence(s) matched."
echo "   (real current GitHub repo / AUR package / upstream attribution /"
echo "    live domain / asset filenames / unregistered social handle)"

echo
echo "== stage 2/4 — case-form census =="
for form in Termul termul TERMUL; do
  count=$(rg -a -o --hidden --case-sensitive "$form" "${SCAN_GLOBS[@]}" . 2>/dev/null | wc -l | tr -d ' ')
  printf '   %-8s %s occurrence(s) repo-wide (whitelisted + registered included)\n' "$form" "$count"
done

if ((RUN_SUITES == 0)); then
  echo
  echo "== stages 3-4 skipped (--no-suites) =="
  exit "$red"
fi

echo
echo "== stage 3/4 — reverse SE_ scan (frozen env-name inventory) =="
# The reverse scan is not a bare `rg 'SE_[A-Z0-9_]+'`: a bare match fires on the
# `SE_` inside `PARSE_FAILED` and `RESPONSE_ID`, and has nothing to compare its
# output against. `env-name-parity.test.ts` walks the same tree with a
# `(?:^|[^A-Za-z0-9])SE_` boundary and diffs the result against the sha256-frozen
# 65-name inventory, so one NEW unregistered `SE_<NAME>` makes the set unequal
# and the test red. Its `SE_FILE_OBJECT` exclusion is itself guarded by a test
# that the name is still imported from `windows_sys`, so the exclusion cannot rot
# into a hiding place.
#
# Note the shape of the placeholder above: this file is scanned by that same
# inventory gate, so spelling a screaming-snake name here would itself register
# as a 66th env name. The angle brackets keep it out of the character class.
if bunx vitest run scripts/tests/env-name-parity.test.ts; then
  echo "   ok — reverse SE_ scan matches the frozen inventory."
else
  echo "   FAIL — reverse SE_ scan diverged from the frozen inventory."
  red=1
fi

echo
echo "== stage 4/4 — FORBID-07 brand-seam thread affinity =="
if (cd src-tauri && cargo test --test brand_seam_thread_affinity); then
  echo "   ok — thread-affinity gate green."
else
  echo "   FAIL — thread-affinity gate red."
  red=1
fi

echo
if ((red)); then
  echo "RESULT: FAIL"
else
  echo "RESULT: PASS"
fi
exit "$red"
