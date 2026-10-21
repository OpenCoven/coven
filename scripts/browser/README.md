# Optional browser verification

Run `node scripts/browser-capability.mjs --check` from the repository root.
Default discovery uses tracked product manifests under `packages/` and `npm/`;
it excludes these tooling and test fixtures. Packages without a declared browser
test report `not-applicable` and require neither a browser nor a lock/parser install.
A direct `@playwright/test` or `@vitest/browser-playwright` dependency, a root
`playwright.config.*`, or a `test:browser` script declares browser tests.
A transitive Playwright dependency alone does not.

For an applicable package, install its dependencies with its frozen lock first.
Browser lock resolution also needs the repository's locked semver and YAML parsers:

```sh
npm ci --prefix scripts/browser --ignore-scripts
node scripts/browser-capability.mjs --install --package packages/example
node scripts/browser-capability.mjs --check --package packages/example
```

`--install` invokes only the installed, lock-matched Playwright core CLI. It
never invokes `npx`, chooses a latest version, installs system packages, or
requests elevation. `--check` never downloads browsers. Both modes launch the
selected browser headlessly, check a local page title, and close it. They do
not execute the package's browser test suite or evaluate its configuration.
Run that package's declared test command separately.

The default smoke browser is Chromium. Select `--browser firefox` or
`--browser webkit` to verify those capabilities explicitly. A Chromium result
makes no assertion about other browsers or browser channels in package config.
Artifacts and revision overrides come from the lock-matched installed
`playwright-core/browsers.json`; no production browser version is hardcoded.
Npm lock versions 2/3 and PNPM lock version 9 are supported. Unsupported, stale,
ambiguous, or incomplete locks fail; installed package versions must match the
resolved dependency chain. PNPM requires an exact `packageManager` pin.

The default cache is `~/.cache/coven/playwright/<core-version>-<lock-hash-prefix>`.
Override it with an absolute `PLAYWRIGHT_BROWSERS_PATH` or `--cache` outside
the package. Use the same cache for provisioning and subsequent tests. Relative
paths (including Playwright's special value `0`) are rejected to avoid
worktree-dependent storage. No cache cleanup or global package mutation occurs.

Each package emits one JSON result on stdout. Commands and the chosen browser
cache are visible on stderr. Any failure makes the process exit nonzero;
other packages are still checked. Raw child-process errors are withheld because
download/proxy messages can contain credentials or query tokens. Stable codes:

| Code | Recovery |
| --- | --- |
| `browser-binary-missing` | Run `--install` with the same lock and cache. |
| `system-libraries-missing` | Provision Playwright's documented OS dependencies through the approved environment setup, then retry. |
| `download-denied` | Result includes the denied hostname and HTTP status when available; have the environment owner authorize that host/proxy route, then retry. |
| `network-failure` | Check the approved proxy, certificates and network route; no unrestricted-network fallback is attempted. |
| `package-not-installed` / `installed-version-mismatch` | Reinstall package dependencies from the frozen lock. |
| `browser-process-timeout` / `browser-process-failed` | Inspect the environment separately; readiness has not been established. |

Install and smoke have independent bounded process timeouts. Library availability
is proven by real launch on this host, rather than inferred from a package list.
See [Playwright browser documentation](https://playwright.dev/docs/browsers) for
browser installation and system dependencies.

## Contract tests and an optional real pilot

```sh
npm ci --prefix scripts/browser --ignore-scripts
node --test scripts/browser-capability.test.mjs
npm ci --prefix scripts/fixtures/browser-npm --ignore-scripts
node scripts/browser-capability.mjs --install --package scripts/fixtures/browser-npm --cache /tmp/coven-browser-pilot
node scripts/browser-capability.mjs --check --package scripts/fixtures/browser-npm --cache /tmp/coven-browser-pilot
```

The committed npm and PNPM fixtures are real generated locks. Unit tests cover
absent declarations, graph resolution, installed-version mismatch, missing
binaries/packages, and sanitized failures. Missing OS libraries and HTTP403 are
simulated in unit tests and are not real environment-denial evidence. The optional
pilot downloads and runs a real browser; ordinary product verification does not
require it when no product declares browser tests.
