# Disposable distro bootstrap contracts

This harness is under development for #1450. It requires the A1 native installer,
A2 Cloud setup scripts, and reviewed CI integration before the matrix can pass.
It does not emulate a managed Cloud VM and never establishes provider access or
human environment publication.

`images.json` pins official Docker Library Debian12 (`bookworm-slim`) and
Ubuntu24.04 (`noble`) manifest-index digests resolved from Docker Registry v2 on
2026-10-20. The test platform is explicitly `linux/amd64`; no ARM execution is
claimed. Tags are recorded only for maintainers; execution uses the digest.
To update, resolve the official tag's manifest digest, review the distro and
Python compatibility, update the inventory, and rerun both fresh contracts.

```sh
python3 scripts/bootstrap-contract/runner-test.py
python3 scripts/bootstrap-contract/runner.py --image debian12 --output /tmp/coven-debian-contract --plan
python3 scripts/bootstrap-contract/runner.py --image debian12 --output /tmp/coven-debian-contract
python3 scripts/bootstrap-contract/runner.py --image ubuntu24 --output /tmp/coven-ubuntu-contract
```

Each real run requires Docker, a clean committed checkout, its dependency entry
points, a new external output directory, and network access already authorized
for the container runner. Missing Docker is a nonzero `docker-unavailable`
result. Plans say `planned`; they do not prove image pulls or test execution.
The runner fetches the exact source commit into a self-contained shallow checkout
and mounts it read-only. The container copies it into disposable writable storage.
No host home or Docker socket is mounted; privileged/host-network mode is unused.
No tool caches are reused across matrix runs. The owned container is removed on
completion or timeout, while requested evidence remains outside the source.

The initial image setup installs Python, Git, certificates, Bash and util-linux
through the image's signed APT configuration. Contract cases then cover actual
native root install, repeated install, unchanged source metadata, and actual
non-root privilege refusal. Unsupported OS, cross-distro and malformed APT cases
use injected metadata and are labelled accordingly. A2 mock tests cover pinned
version/network/build failures; they remain labelled mock evidence even inside
a real container. Full A2 bootstrap runs twice with a shared container-local
cache, followed by readiness, retaining each report. Operational readiness and
provider authentication are never inferred from development readiness.

Only structured `*.json` evidence is suitable for CI upload. Raw Docker stdout
and stderr stay local to the requested output folder; they must not be uploaded
or printed because client proxy/registry diagnostics may contain sensitive data.
Failed bootstrap/readiness JSON reports are retained as well as passing reports.
No code broadens network policy, repairs source configuration, suppresses a
failure, or substitutes local fixtures for real Cloud evidence.

The CI matrix is routed by the `bootstrap` change category: bootstrap/cloud
scripts, Cargo metadata, package manifests/locks, and workflow/toolchain changes.
The matrix uses two independent jobs (`fail-fast: false`), no cross-run cache,
and a 95-minute job limit around the runner's 90-minute container limit. Its
result is a dependency of `PR gate`; routed failures are not optional. Ordinary
unrelated documentation/package-source edits do not enable a fresh tool install.
