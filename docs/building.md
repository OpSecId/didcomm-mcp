# Building didcomm-mcp from source

The [release archives](install.md#1-download) are the easiest way to get didcomm-mcp.
Build it yourself to run an unreleased change, or a platform there's no archive for.

- [What you need](#what-you-need)
- [`cargo install`](#cargo-install)
- [From a clone](#from-a-clone)
- [Static and cross-platform builds](#static-and-cross-platform-builds)
- [A container image](#a-container-image)
- [Troubleshooting](#troubleshooting)

## What you need

- **Rust 1.95 or newer**, from [rustup](https://rustup.rs): `rustup update stable`.
- **A C compiler.** The cryptography library (aws-lc, through rustls) compiles some C.
  - Linux: `gcc` or `clang` (Debian/Ubuntu: `sudo apt install build-essential`; Fedora:
    `sudo dnf install gcc`).
  - macOS: the Xcode command line tools (`xcode-select --install`).
  - Windows: the Visual Studio Build Tools with the "Desktop development with C++"
    workload (rustup offers to install them).

  On platforms aws-lc has no prebuilt bindings for, it also needs CMake. The six
  [release targets](install.md#1-download) don't.
- **Git**, only to build from a clone.

## `cargo install`

```sh
cargo install --locked --git https://github.com/wyvrn-cloud/mcp --tag v0.1.1 didcomm-mcp
```

This builds the release tagged `v0.1.1` and puts the binary in `~/.cargo/bin`
(`%USERPROFILE%\.cargo\bin` on Windows), which rustup adds to your `PATH`.

- **Always pass `--locked`.** It builds with the exact dependency versions in the
  repository's `Cargo.lock`, the ones the release was tested with. Without it, cargo
  picks the newest versions it's allowed to, which can be untested combinations.
- `--tag` picks a [release](https://github.com/wyvrn-cloud/mcp/releases). Use
  `--branch master` instead for the latest unreleased code, or `--rev <commit>` for a
  specific commit.
- To upgrade, run the same command with the new tag. Reinstalling the same version needs
  `--force`.
- `cargo uninstall didcomm-mcp` removes it.

Then connect your MCP host to it ([install.md, section 2](install.md#2-connect-an-mcp-host)),
using `didcomm-mcp` (or the full path, `~/.cargo/bin/didcomm-mcp`) as the command.

**As a service:** a `--user` service can run the binary from `~/.cargo/bin`. A
system service on Linux can't see home directories, so copy the binary out first, then
install the service from the copy:

```sh
sudo install -m 755 ~/.cargo/bin/didcomm-mcp /usr/local/bin/
sudo /usr/local/bin/didcomm-mcp service install
```

On macOS and Windows a system service can run it from where cargo put it, but it then
breaks if you `cargo uninstall` or clean up that directory; copying it to
`/usr/local/bin` or `C:\Program Files\didcomm-mcp\` is safer there too. After an upgrade,
copy the new binary over the old one and run `service install` again (see
[install.md, section 5](install.md#5-upgrade)).

## From a clone

```sh
git clone https://github.com/wyvrn-cloud/mcp
cd mcp
cargo build --release --locked
./target/release/didcomm-mcp --version
```

The binary is `target/release/didcomm-mcp` (`didcomm-mcp.exe` on Windows); copy it
wherever you like, or install it from the clone with `cargo install --locked --path .`.
`git checkout v0.1.1` first to build a release instead of `master`.

Run the tests with `cargo test`. `e2e/run.py` runs the whole stack in containers (see the
[README](../README.md#end-to-end-in-containers)).

## Static and cross-platform builds

The Linux release archives are static (musl) binaries, which run on any distribution.
To build one:

```sh
sudo apt install musl-tools                    # musl-gcc, for the C code
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl
# target/x86_64-unknown-linux-musl/release/didcomm-mcp
```

Use `aarch64-unknown-linux-musl` on an ARM64 machine. On a Mac, build either
architecture from the other (`rustup target add x86_64-apple-darwin`, then
`--target x86_64-apple-darwin`). For Windows, the releases add
`RUSTFLAGS="-C target-feature=+crt-static"` so the binary doesn't need the Visual C++
runtime installed.

[`.github/workflows/release.yml`](../.github/workflows/release.yml) builds every release
target this way, and is the reference for the exact steps.

## A container image

```sh
docker build -t didcomm-mcp .
```

See the [README](../README.md#running-it) for running it, and the
[`Dockerfile`](../Dockerfile) for the build secrets a TLS-intercepting proxy needs.

## Troubleshooting

- **`rustc 1.xx is not supported by the following packages`**: update Rust,
  `rustup update stable`.
- **`failed to find tool "cc"`, or a C compile error in `aws-lc-sys`**: install the C
  compiler above. On Windows, run the build from a terminal that has the Build Tools
  (e.g. "Developer PowerShell for VS").
- **The build works but the server misbehaves in ways a release doesn't**, such as errors
  talking to the registry or a mediator: check you built with `--locked`. A build
  without it can pick up dependency versions that were never tested together.
- **`failed to get ... as a dependency` / `failed to fetch` for `didcomm`**: cargo
  fetches the `didcomm` dependency from GitHub. If git itself can reach GitHub (e.g. it's
  set up for your proxy), set `CARGO_NET_GIT_FETCH_WITH_CLI=true` so cargo fetches
  through it.
