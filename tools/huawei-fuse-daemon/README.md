# Huawei FUSE Daemon

Status: abandoned experiment. Huawei Cloud Gallery's web API access is not
stable enough for product integration because direct API calls can return empty
responses unless the loaded web page performs the request. This branch keeps
the Rust/FUSE proof of concept for reference, but the feature should not be
promoted or integrated into the photo viewer.

Standalone daemon that exposes Huawei Cloud Gallery as a read-only FUSE
filesystem. The photo viewer app is not started or modified by this daemon; it
can scan the mount path later as a normal local directory.

## Build

```bash
cargo build --release
```

This crate uses `fuser` without the `libfuse` feature, so it does not require
`libfuse3-dev`. The host still needs `/dev/fuse` and `fusermount3` at runtime.

## First Login

Run this only when the profile has no valid Huawei Cloud session or the session
has expired:

```bash
cargo run --release -- --login
```

The login profile is stored at:

```text
~/.local/share/photoViewer/auth/huawei-cloud-profile
```

The daemon does not store the account password. It reuses Chromium's profile
cookies and browser state.

## Mount

```bash
cargo run --release --
```

Defaults:

```text
mount:   ~/.local/share/photoViewer/mounts/huawei-cloud
cache:   ~/.cache/photoViewer/remotes/huawei-cloud
profile: ~/.local/share/photoViewer/auth/huawei-cloud-profile
```

Mount mode waits up to 300 seconds for the browser profile to become
authenticated. For first-time interactive testing, use:

```bash
cargo run --release -- --visible
```

Use `--auth-timeout SECONDS` to change that wait window.

The exposed tree maps Huawei Cloud albums to directories:

```text
Camera/
  <remote-id-prefix>_<index>.jpg
Screenshots/
  <remote-id-prefix>_<index>.jpg
<album-name>/
  <remote-id-prefix>_<index>.jpg
```

Startup only loads the album list. Listing an album directory loads that
album's image entries on demand, and opening or copying a file triggers FUSE
`read`; the daemon downloads the cloud media, caches it, and returns bytes to
the caller.

## Useful Overrides

```bash
cargo run --release -- \
  --mount /tmp/huawei-cloud-mount \
  --cache /tmp/huawei-cloud-cache \
  --profile /tmp/huawei-cloud-profile \
  --count 80
```

`--count` is the per-album page size used when an album directory is first
loaded.

If direct HTTP media download is rejected by Huawei's proxy, the daemon falls
back to fetching bytes inside the authenticated Chromium page context while
still returning data through the FUSE read path.
