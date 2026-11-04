## Developer Build Guide

## Design Philosophy

- Code and data structures must be future-proof
- Don't interrupt the user
- Errors should not create stoppages
- No application or format version numbering

**Quick Notes:**
 - This should compile and work on all OS, but has only been tested in Windows/Linux
 - Memory and CPU loads have only basic Optimization; Runs very well for me using only 67MB of memory (YMMV)
 - UI is intentionally simple, CTRL+/CTRL- are your friend
 - Build-init script configures the entire build system, including downloading required artifacts
 - This prject assumes MSYS/GNU Toolchain
 - I'm not an audio engineer, so feel free to submit any improvements which affect accuracy and quality

### Validation and probe policy

- Do not add inline Rust tests. All probes must be standalone `.rs` programs
  that are compiled and run explicitly, not shell, PowerShell, or other scripts.
- Keep temporary test and validation probes, including their source,
  binaries, and fixtures, under the ignored `_local` tree, not in Rust crate
  source directories. These artifacts are disposable and may be deleted at any time.
- Run probes independently of builds. Builds must not invoke probes or gate
  compilation on test or probe results.

This policy applies to probes, not production build or packaging scripts.

### Windows MSYS prerequisites

The Windows build uses a POSIX shell and GNU userland tools for the vendored `tikv-jemalloc-sys` configure/make step. Install MSYS2 and the MinGW64 toolchain before running `build-init.ps1`.

Recommended MSYS2 packages:

```powershell
pacman -S --needed base-devel mingw-w64-x86_64-toolchain
```

That gives you the tools this build expects, including `sh`, `bash`, `sed`, `grep`, `expr`, `gawk`, `make`, `ar`, `nm`, and `install`.

Make sure one of these shell locations is available so the bootstrap script can discover it automatically:

- `C:\msys64\usr\bin\sh.exe`
- `C:\Program Files\Git\bin\sh.exe`
- `C:\Program Files\Git\usr\bin\sh.exe`

If MSYS2 is installed somewhere else, set `MSYS2_ROOT` to the MSYS2 install root before running the bootstrap script. The build also uses the same shell path for `CONFIG_SHELL`, `SHELL`, and `MAKESHELL` during jemalloc configuration.


## Clone and bootstrap

If you are setting up the Windows development environment for the first time, install the MSYS2 packages above first, then continue here.

```powershell
git clone https://github.com/cmykflutterby/flutzPlayer.git
cd flutzPlayer
.\build-init.ps1
```

This prepares the local dev layout, vendors the required external crate sources, and stages the workspace so the regular build scripts can run without manual setup.

To return the checkout to a pre-initialized state, run:

```powershell
.\build-init.ps1 -Clean
```

## First build/validation flow

```powershell
cargo check -p flutzplayer
.\build.ps1 -Configuration Debug -BuildDat
```

Run from source:

```powershell
cargo run -p flutzplayer -- --gui --data-dir drops/flutzplayer/data
```

Run packaged drop:

```powershell
.\drops\flutzplayer\flutzplayer-debug.exe --gui --data-dir .\drops\flutzplayer\data
```

On Linux, an AppImage uses the `data` folder next to the original AppImage,
regardless of the current working directory. Development executables use the
`data` folder beside the executable. `--data-dir` overrides either default.

The application creates `data/preferences.ini` on first GUI launch. If the
directory is not writable, bundled DAT files remain readable and the status bar
reports that settings cannot be saved; move the application to a user-owned
location such as `~/Applications/flutzPlayer`, or select a writable `--data-dir`.
Preferences are written atomically through SDL3 file I/O (`flutz_fs_sdl3`).

On its first Linux AppImage GUI launch, flutzPlayer installs a per-user
application-menu entry and icon under `$XDG_DATA_HOME` (or
`~/.local/share`). Launch the AppImage directly after moving it to update the
menu entry. Integration failures are logged and do not prevent the GUI from
opening; development executables and command-line tools do not register a
launcher. Direct desktop integration does not support `%` or `=` in the
AppImage's own path; move it to a path without those characters and launch it
directly to register it. The AppDir resources for packaging are
[`flutzplayer.desktop`](build/linux/appimage/flutzplayer.desktop) and
[`flutzplayer.png`](build/linux/appimage/flutzplayer.png).

To run the Linux desktop-integration probe directly on the build/test machine
or VM, install `desktop-file-validate`, `gio`, `xvfb-run`, `xdotool`, and `jq`, and
run as an unprivileged user:

```sh
cargo build -p flutzplayer --bin flutzplayer --example desktop_integration_probe
target_dir="$(cargo metadata --no-deps --format-version 1 | jq -r .target_directory)"
xvfb-run -a "$target_dir/debug/examples/desktop_integration_probe" \
  "$target_dir/debug/flutzplayer" build/linux/appimage/flutzplayer.png \
  "$HOME/_local/desktop-integration-probes"
```

The probe uses isolated XDG and home directories and does not change the
user's real desktop launcher.

To also validate a real AppImage with DAT files in its adjacent `data`
directory, build the separate real-AppImage probe and pass its path:

```sh
cargo build -p flutzplayer --example appimage_desktop_probe
target_dir="$(cargo metadata --no-deps --format-version 1 | jq -r .target_directory)"
xvfb-run -a "$target_dir/debug/examples/appimage_desktop_probe" \
  "/absolute/path/to/flutzPlayer-x86_64.AppImage" \
  build/linux/appimage/flutzplayer.desktop build/linux/appimage/flutzplayer.png \
  "$HOME/_local/appimage-desktop-probes"
```

It copies the AppImage into an isolated fixture and tests direct and extraction
launches, the generated launcher, moving the folder, and integration failures.


Build DAT fileset:

```powershell
.\build-dat.ps1
```

Create build:

```powershell
.\build.ps1 -Configuration Release
```

Create build with fresh DATs:

```powershell
.\build.ps1 -Configuration Release -BuildDat
```

### Linux build destinations

After provisioning the VM with `.\build-init.ps1 -OS Linux`, build a plain
Linux executable and its populated `data` directory without AppImage packaging:

```powershell
.\build.ps1 -OS Linux -Configuration Debug
.\build.ps1 -OS Linux -Configuration Release
```

These builds go to `drops\flutzPlayer-Linux` (`flutzplayer-debug` for Debug,
`flutzplayer` for Release). Use `-SkipBuild` to stage existing guest binaries.
`-CleanDrop` clears only this destination while preserving the drop and `data`
directory identities. Without a DAT switch, the build stages the existing
files from `_local\generated-assets\dat`, generated separately with
`.\build-dat.ps1` or a previous Linux DAT build.

Remote guest commands have a four-hour timeout; transfers have a 30-minute
timeout; SSH readiness checks time out after 30 seconds. Override them for a
slow or constrained VM with `-LinuxCommandTimeoutSeconds`,
`-LinuxTransferTimeoutSeconds`, and `-LinuxSshTimeoutSeconds`. Commands stream
to the terminal and are retained under `_local\linux\build-output`, including
failed guest command logs. The build expects the provisioned VM to be running.

To rebuild DATs natively in the Linux VM:

```powershell
.\build.ps1 -OS Linux -Configuration Release -BuildDat
# -RebuildDat is equivalent; -SkipBuild may be combined with either switch.
```

`-BuildDat` and `-RebuildDat` are equivalent on both Linux selectors;
`-SkipBuild` skips only the application workspace build, and `-NoJemallocMemory`
opts out of the default jemalloc feature. Linux-AppImage is Release-only and
uses `cargo --locked`; plain Linux intentionally keeps the normal Cargo lockfile
behavior, so those selectors are not exact command-line parity.

The host runs the existing soundfont bootstrap before source synchronization.
The guest builds the release DAT packer, packs and validates the real manifest,
and inspects the numbered family. The application executable and native packer
are transferred to host staging and SHA-256 verified before publication. Only
after checksum-verified DAT transfers are the generated files replaced in
`_local\generated-assets\dat` and staged into the drop. `-SkipBuild` skips the
workspace/application build, but a requested DAT rebuild still builds or reuses
the release packer. The dropped native packer matches the selected Debug or
Release configuration.

The Linux drop also contains `dat-packer/dat-packer`, `pack-dat.sh`, and
`dat-manifest.toml`. After transferring the drop from Windows to Linux, restore
the executable bits if the transfer did not preserve them:

```sh
chmod +x dat-packer/dat-packer dat-packer/pack-dat.sh
./dat-packer/pack-dat.sh --base-dir "/path/containing/soundfonts"
```

Manifest `source_path` values resolve under `--base-dir`; place the required
`soundfonts/` files there before regenerating DATs. The default base directory
is `dat-packer`, and the default output is the sibling `data/assets.dat` family,
not `dat-packer/data`. Use `--manifest`, `--output`, `--base-dir`,
`--chunk-size`, and `--max-file-size` to customize paths and byte limits.
`--number-single` gives a one-file output a `-000.dat` suffix; `--help` lists
the options. Missing source files are reported by the packer.

The wrapper deletes the existing output DAT family, packs directly into the
output directory, and inspects the result. Unrelated files are left alone. Close
applications using the DAT family before regenerating it; if packing fails,
fix the cause and rerun it.

Linux binary validation rejects shared SDL3/xkbcommon dependencies and checks
defined core, compose, and X11 xkbcommon function symbols in the build artifact.
Embedded library-name strings alone are not evidence of dynamic loading:
Debug builds can retain them in panic messages even with static linkage.

### Linux AppImage build

Build the release AppImage, its final release archive, and the loose drop:

```powershell
.\build.ps1 -OS Linux-AppImage -Configuration Release
.\build.ps1 -OS Linux-AppImage -Configuration Release -BuildDat
```

The guest assembles one distribution tree and archives it as
`flutzPlayer-linux-x86_64.tar.gz`:

```text
flutzPlayer/
├── flutzPlayer-x86_64.AppImage
├── data/assets-*.dat
└── dat-packer/
    ├── dat-packer
    ├── pack-dat.sh
    └── dat-manifest.toml
```

The archive and its sibling `flutzPlayer-linux-x86_64.tar.gz.sha256.txt`
(`sha256sum -c` format, not stored inside the archive) are published to
`drops\flutzPlayer-Linux-Release`. The loose drop `drops\flutzPlayer-AppImage`
is populated by extracting that same verified archive, so its contents equal the
archive contents; generated preferences are never shipped. Publication reuses
the existing drop, `data`, and release directories, preserving their Dropbox
identity, permissions, and sharing configuration. A normal build removes every
other file and folder from the AppImage drop except runtime
`data\preferences.ini`, then verifies that the drop holds exactly the archive's
files. `-CleanDrop` also removes the preferences and clears the release
directory while retaining those directories; it never clears the plain Linux
drop.

The archive uses name-sorted entries, `root:root` (0:0) ownership, modes
`0755` for directories, the AppImage, `dat-packer`, and `pack-dat.sh` and
`0644` for other files, a single timestamp taken from the AppImage build
metadata, and a gzip header without name or time. Before transfer, the guest
verifies the checksum and extracts the archive into a fresh directory to check
its exact tree, modes, and ownership, the absence of preferences and build
files, AppImage data discovery (`--appimage-extract-and-run --inspect-dat`), and
the packer (`dat-packer --inspect`, `pack-dat.sh --help`).

AppImage packaging requires Release configuration. `-SkipBuild` reuses a
previously recorded AppImage release build only when its synchronized source
snapshot, Cargo lockfile, Rust toolchain, binary checksum, and allocator choice
still match. Otherwise, rebuild without `-SkipBuild`. `-NoJemallocMemory`,
`-BuildDat`, and `-RebuildDat` have the same meaning as for the plain Linux
build. AppImage builds use `cargo --locked`; plain Linux builds do not add that
flag. Existing cached DAT parts are copied with checksum verification and
inspected by the native Linux packer before packaging.

The guest runs both packaging tools using extraction rather than FUSE, verifies
the pinned Type 2 runtime and extracted library policy, and records the tool
identities and source/build hashes in `_local\linux\build-output` alongside
the packaging log. Build outputs, caches, and drops are disposable: a failed or
interrupted build can leave them partially updated, and there is no rollback or
recovery. Rerun the build, using `-CleanDrop` when a fresh drop is wanted.

On Linux, verify and extract the release archive (tar preserves executable
bits). Normal launch uses FUSE3; the second command is the no-FUSE extraction
fallback:

```sh
sha256sum -c flutzPlayer-linux-x86_64.tar.gz.sha256.txt
tar -xzf flutzPlayer-linux-x86_64.tar.gz
cd flutzPlayer
./flutzPlayer-x86_64.AppImage --gui
./flutzPlayer-x86_64.AppImage --appimage-extract-and-run --gui
```

When copying the loose drop instead, restore the executable bits with
`chmod +x flutzPlayer-x86_64.AppImage dat-packer/dat-packer dat-packer/pack-dat.sh`.

#### Updating packaging-tool pins

Update the immutable linuxdeploy and appimagetool release identities, URLs,
and SHA-256 values in `build/linux/tool-versions.json` from verified upstream
release assets. The Type 2 runtime is the prefix of the pinned appimagetool
asset, not a mutable runtime download. Save the new, checksum-verified
appimagetool asset as `$HOME/_local/appimagetool-verified.AppImage`, then
inspect its runtime on Linux:

```sh
tool="$HOME/_local/appimagetool-verified.AppImage"
mkdir -p "$HOME/_local"
chmod +x "$tool"
runtime="$HOME/_local/appimage-runtime-inspection"
size=$("$tool" --appimage-offset)
head -c "$size" "$tool" > "$runtime"
chmod +x "$runtime"
sha256sum "$runtime"
"$runtime" --appimage-version
objdump -h "$runtime" | grep '\.digest_md5'
```

Set `appimageRuntime` to the verified source asset/release, complete runtime
commit, prefix size and SHA-256, and the decimal offset and size of the
`.digest_md5` section. Resolve an abbreviated runtime commit against the
upstream type2-runtime repository. Run `.\build-init.ps1 -OS Linux` to
refresh the pinned tools, then rebuild and verify both launch modes.

`.\build-init.ps1 -OS Linux -Clean` clears **both** Linux drop
directories and synchronized guest build state while preserving the VM and
caches.

**NOTES:**

- [build.ps1](build.ps1): workspace build + drop staging
- [build-dat.ps1](build-dat.ps1): DAT generation from [assets/dat-manifest.toml](assets/dat-manifest.toml)
