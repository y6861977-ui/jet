# jet

A package manager for JavaScript that keeps working when the network doesn't.

I'm a developer from Turkmenistan. Here npm and a lot of registries and CDNs are
often blocked, or they just drop in the middle of an install. The same thing
happens in Russia, Iran and plenty of other places. So I wrote jet: it's
npm-compatible, it's a single small binary with no Node needed, and it's built
to survive a bad connection instead of giving up on it.

## What it does

- **Switches mirrors automatically** when a registry is blocked or unreachable.
- **Installs from local cache** when you're offline.
- **Resumes a dropped download** instead of starting the whole thing over.
- **npm-compatible** — reads `package.json`, writes a lockfile, works with the
  npm registry and private registries.
- **One binary, zero runtime dependencies.** No Node required to run jet itself.

It's written in Rust with the standard library only — no third-party crates.

## Install

**macOS / Linux**

```sh
curl -fsSL https://github.com/y6861977-ui/jet-dist/releases/latest/download/install.sh | sh
```

**Windows**

Download `jet-setup.exe` from the [latest release](https://github.com/y6861977-ui/jet-dist/releases/latest)
and run it, or grab the raw `jet-windows-x64.exe`.

## Usage

```sh
jet install              # install everything in package.json
jet install react        # add a package
jet uninstall react      # remove one
jet install -g <pkg>     # install globally
jet tree                 # show the dependency tree
jet audit                # scan installed packages for known CVEs (OSV.dev)
jet registry check       # check the registry / mirrors
jet help                 # all commands
```

## Open core

jet is open-core. Everything that solves the network problem — install, resolve,
mirror switching, cache, offline, resume, `jet audit` — is in this repository and
free, under the MIT license. A few extra features (package transparency with
`jet explain` / `jet diff`, and the private-registry server for teams) are part
of **jet Pro** and are not in this repo. The free tool is complete on its own;
Pro is optional. See https://jet.kim.

## Build from source

```sh
cargo build --release
# binary at target/release/jet
```

Requires a recent Rust toolchain (edition 2024). No other dependencies.

## License

MIT — see [LICENSE](LICENSE).
