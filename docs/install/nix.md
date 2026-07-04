---
summary: "Reproducible Coven environment with Nix flakes."
read_when:
  - You use Nix to manage tooling
title: "Nix"
description: "Install Coven with Nix: a reproducible flake-based setup that pins the daemon, CLI, and supported harnesses across hosts and developer machines."
---

# Nix

Use Nix to pin build prerequisites and harness tooling around a source checkout. This repository does not currently use this page to promise an official Coven flake output.

For the shortest install, use [Install via npm](/install/npm). For reproducible development shells, use the pattern below.

## Development shell

Create a local `flake.nix` in your own workspace:

```nix
{
  description = "Coven development shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          pkgs.rustc
          pkgs.cargo
          pkgs.pkg-config
          pkgs.openssl
          pkgs.nodejs_22
          pkgs.git
        ];
      };
    };
}
```

Enter the shell and build from source:

```sh
nix develop
git clone https://github.com/OpenCoven/coven.git
cd coven
cargo build --workspace
cargo run -p coven-cli -- doctor
```

## Harness setup

Install harness CLIs inside the environment where you run Coven, or add them to your Nix shell when available from your package set.

For npm-managed harnesses:

```sh
npm install -g @openai/codex
codex login
npm install -g @anthropic-ai/claude-code
claude doctor
coven doctor
```

## State isolation

Use an explicit state directory per Nix shell or host:

```sh
export COVEN_HOME="$PWD/.coven-state"
coven doctor
```

Do not commit `.coven-state` or any other `COVEN_HOME` contents.

## Related

- [Install from source](/install/from-source)
- [Install via cargo](/install/cargo)
- [COVEN_HOME layout](/daemon/coven-home)
