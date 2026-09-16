---
description: Install stagelint from whichever package manager your project already uses, or as a
  prebuilt binary.
---

# Installation

stagelint is a single binary with no runtime. Install it however your project already installs
tools, or download a prebuilt binary directly.

## Node

::: code-group

```sh [npm]
npm install --save-dev @stagelint/stagelint
```

```sh [pnpm]
pnpm add -D @stagelint/stagelint
```

```sh [yarn]
yarn add --dev @stagelint/stagelint
```

```sh [bun]
bun add --dev @stagelint/stagelint
```

```sh [deno]
deno add --dev @stagelint/stagelint
```

:::

If you do not already use a [hook manager](/hook-managers/), add the hook to your `prepare` script
so it installs itself for the whole team:

```json
{
  "scripts": {
    "prepare": "stagelint init"
  }
}
```

## Python

Add it to your project:

::: code-group

```sh [uv]
uv add --dev stagelint
```

```sh [Poetry]
poetry add --dev stagelint
```

:::

Or install it globally:

::: code-group

```sh [uv]
uv tool install stagelint
```

```sh [pipx]
pipx install stagelint
```

```sh [pip]
python -m pip install --user stagelint
```

:::

## Rust

Download a prebuilt binary using `cargo-binstall`:

```sh
cargo binstall stagelint
```

Or compile it from source:

```sh
cargo install stagelint
```

## Homebrew

```sh
brew install abemedia/tap/stagelint
```

## Scoop

```sh
scoop bucket add abemedia https://github.com/abemedia/scoop-bucket
scoop install stagelint
```

## Nix

```sh
nix profile add nur#repos.abemedia.stagelint
```

Alternatively, add NUR using its
[installation guide](https://github.com/nix-community/NUR#installation), then add stagelint to your
configuration:

```nix
{ pkgs, ... }: {
  home.packages = with pkgs; [
    nur.repos.abemedia.stagelint
  ];
}
```

## mise

```sh
mise use aqua:abemedia/stagelint
```

## Devbox

```sh
devbox add nur#repos.abemedia.stagelint
```

## Install script

Download, verify and install a prebuilt binary:

```sh
curl -fsSL https://stagelint.dev/install.sh | sh
```

It installs to `/usr/local/bin` when that is writable, otherwise to `~/.local/bin`. Set
`STAGELINT_INSTALL_DIR` to choose the directory, or `STAGELINT_VERSION` to install a specific
version:

```sh
curl -fsSL https://stagelint.dev/install.sh | STAGELINT_VERSION=%version% sh
```

## Manual install

Download a prebuilt binary or Linux package from the
[release page](https://github.com/abemedia/stagelint/releases/latest).
