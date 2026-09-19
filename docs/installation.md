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

## Go

Add it to your project and run it with `go tool stagelint`:

```sh
go get -tool github.com/abemedia/stagelint
```

Or install it globally:

```sh
go install github.com/abemedia/stagelint@latest
```

## .NET

```sh
dotnet tool install -g stagelint
```

Needs the .NET 10 SDK or later.

## Homebrew

```sh
brew install abemedia/tap/stagelint
```

## Scoop

```sh
scoop bucket add abemedia https://github.com/abemedia/scoop-bucket
scoop install stagelint
```

## APT

```sh
sudo curl -fsSL https://pkg.stagelint.dev/deb/key.asc -o /etc/apt/keyrings/stagelint.asc
sudo chmod a+r /etc/apt/keyrings/stagelint.asc
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/stagelint.asc] https://pkg.stagelint.dev/deb stable main" | sudo tee /etc/apt/sources.list.d/stagelint.list > /dev/null
sudo apt update
sudo apt install stagelint
```

## DNF

```sh
echo '[stagelint]
name=stagelint
baseurl=https://pkg.stagelint.dev/rpm/
enabled=1
gpgcheck=0
repo_gpgcheck=1
gpgkey=https://pkg.stagelint.dev/rpm/repodata/repomd.xml.key' | sudo tee /etc/yum.repos.d/stagelint.repo > /dev/null

sudo dnf install stagelint
```

## Pacman

```sh
curl -fsSL https://pkg.stagelint.dev/arch/key.asc | sudo pacman-key --add -
sudo pacman-key --lsign-key 04ECDE59ADB67FFCF334D52A43BD4A43FBABEF34
echo '[stagelint]
Server = https://pkg.stagelint.dev/arch/$arch' | sudo tee -a /etc/pacman.conf
sudo pacman -Syu stagelint
```

## Zypper

```sh
sudo zypper addrepo --refresh https://pkg.stagelint.dev/rpm/ stagelint
sudo zypper --gpg-auto-import-keys refresh
sudo zypper install stagelint
```

## APK

```sh
wget -qO /etc/apk/keys/info@stagelint.dev.rsa.pub https://pkg.stagelint.dev/alpine/info@stagelint.dev.rsa.pub
echo 'https://pkg.stagelint.dev/alpine' >> /etc/apk/repositories
apk update
apk add stagelint
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
