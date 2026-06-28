#!/usr/bin/env bash
# Incrementally build in the GNOME 50 SDK, then run in the PhotoViewer sandbox.
#
# This is the development runner. It avoids flatpak-builder for the edit/run
# loop because flatpak-builder recreates the module build root often enough to
# throw away Cargo's registry and target caches. Building directly in the SDK
# keeps incremental artifacts while using the GNOME 50 toolchain:
#
#   - target/flatpak-debug        (compiled Rust artifacts)
#   - ~/.cache/photoViewer/cargo-home (Cargo registry/git downloads)
#
# The binary is then launched through the installed org.gnome.PhotoViewer app
# sandbox, not the SDK sandbox. GNOME 50's gdk-pixbuf/glycin image loaders fail
# when nested under the generic SDK app-id, which makes thumbnails stay white.
#
# Default behavior is quiet: no additional application logs are printed.
#
# Logging options:
#   --log-domain <module[=level]>       Enable one specific log target (repeatable).
#   -l <module[=level]>                 Alias for --log-domain.
#   --log-domains <d1,d2[=level],...>   Enable multiple targets.
#   -L <comma list>                     Alias for --log-domains.
#   --log-all                           Enable trace logging for all targets.
#   -a                                  Alias for --log-all.
#   --clean                             Remove target/flatpak-debug before running.
#   -c                                  Alias for --clean.
#   --no-audio                          Start without pulseaudio socket binding.
#
# The legacy env var CLEAN=1 is still supported for clean rebuilds.
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: ./run-flatpak.sh [--clean|-c] [--release|-r] [--log-domain|-l <module[=level]> ...] [--log-domains|-L <comma list>] [--log-all|-a] [--no-audio]

Options:
  --clean, -c                     Remove target/flatpak-debug before build
  --release, -r                   Build and run optimized (release) instead of debug.
                                  The extract-heavy scan is CPU-bound, so debug is
                                  ~8x slower; release is the realistic experience.
  --log-domain, -l <target>       Set one rust log domain (repeatable)
  --log-domains, -L <list>        Set multiple domains as comma-separated list
  --log-all, -a                   Print all logs at trace level
  --no-audio                       Start app without pulseaudio socket binding
  -h, --help                      Show this help

Examples:
  ./run-flatpak.sh
  ./run-flatpak.sh -r                       # optimized build (recommended for perf checks)
  ./run-flatpak.sh -l photo_viewer=debug
  ./run-flatpak.sh -l photo_viewer=trace -l photos_page=debug
  ./run-flatpak.sh -L photo_viewer,photos_page=trace
  ./run-flatpak.sh -a
USAGE
}

cd "$(dirname "$0")"

PROJECT_DIR="$(pwd)"
CACHE_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}/photoViewer"
CARGO_HOME_DIR="$CACHE_ROOT/cargo-home"
TARGET_DIR="target/flatpak-debug"
FLATPAK_APP_ID="org.gnome.PhotoViewer"
RUN_WITH_AUDIO=1
RELEASE_BUILD=0

mkdir -p "$CARGO_HOME_DIR"

CLEAN_FLAG="${CLEAN:-}"
PRINT_ALL_LOGS=0
RUST_LOG_DOMAINS=()

while (( "$#" )); do
    case "$1" in
        --no-audio)
            RUN_WITH_AUDIO=0
            shift
            ;;
        -c)
            CLEAN_FLAG=1
            shift
            ;;
        --clean)
            CLEAN_FLAG=1
            shift
            ;;
        -r)
            RELEASE_BUILD=1
            shift
            ;;
        --release)
            RELEASE_BUILD=1
            shift
            ;;
        -l)
            if (( $# < 2 )); then
                echo "Missing argument for -l/--log-domain" >&2
                exit 1
            fi
            RUST_LOG_DOMAINS+=("$2")
            shift 2
            ;;
        --log-domain)
            if (( $# < 2 )); then
                echo "Missing argument for --log-domain" >&2
                exit 1
            fi
            RUST_LOG_DOMAINS+=("$2")
            shift 2
            ;;
        -L)
            if (( $# < 2 )); then
                echo "Missing argument for -L/--log-domains" >&2
                exit 1
            fi
            IFS=',' read -r -a parsed <<< "$2"
            for domain in "${parsed[@]}"; do
                if [[ -n "$domain" ]]; then
                    RUST_LOG_DOMAINS+=("$domain")
                fi
            done
            shift 2
            ;;
        --log-domains)
            if (( $# < 2 )); then
                echo "Missing argument for --log-domains" >&2
                exit 1
            fi
            IFS=',' read -r -a parsed <<< "$2"
            for domain in "${parsed[@]}"; do
                if [[ -n "$domain" ]]; then
                    RUST_LOG_DOMAINS+=("$domain")
                fi
            done
            shift 2
            ;;
        --log-all)
            PRINT_ALL_LOGS=1
            shift
            ;;
        -a)
            PRINT_ALL_LOGS=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage
            exit 1
            ;;
    esac
done

# Resolve the cargo profile once: it drives both the build flag and the binary
# path under $TARGET_DIR (debug/ vs release/). Both profiles coexist under the
# same target root, so toggling does not force a full recompile of the other.
if (( RELEASE_BUILD )); then
    CARGO_PROFILE="release"
    CARGO_RELEASE_FLAG="--release"
else
    CARGO_PROFILE="debug"
    CARGO_RELEASE_FLAG=""
fi

build_log_targets() {
    if (( PRINT_ALL_LOGS )); then
        echo "trace"
        return
    fi

    if (( ${#RUST_LOG_DOMAINS[@]} == 0 )); then
        echo ""
        return
    fi

    local normalized=()
    local domain
    for domain in "${RUST_LOG_DOMAINS[@]}"; do
        if [[ "$domain" == *=* ]]; then
            normalized+=("$domain")
        else
            normalized+=("$domain=debug")
        fi
    done

    local joined="${normalized[0]}"
    for domain in "${normalized[@]:1}"; do
        joined+=",$domain"
    done

    echo "$joined"
}

RUST_LOG_VALUE="$(build_log_targets)"

fix_flatpak_pulse_socket() {
    if (( RUN_WITH_AUDIO == 0 )); then
        return
    fi

    local runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
    local flatpak_pulse_path="$runtime_dir/.flatpak/$FLATPAK_APP_ID/xdg-run/pulse"

    if [ ! -e "$flatpak_pulse_path" ]; then
        return
    fi

    if [ -L "$flatpak_pulse_path" ]; then
        local link_target
        link_target="$(readlink "$flatpak_pulse_path" 2>/dev/null || true)"
        if [ "$link_target" != "../../flatpak/pulse" ]; then
            rm -f "$flatpak_pulse_path"
        fi
    else
        rm -rf "$flatpak_pulse_path"
    fi
}

if [ -n "$CLEAN_FLAG" ]; then
    echo "==> clean rebuild requested; removing $TARGET_DIR..."
    rm -rf "$TARGET_DIR"
fi

echo "==> cargo build ($CARGO_PROFILE) in GNOME 50 SDK sandbox..."
flatpak run \
    --devel \
    --share=network \
    --filesystem="$PROJECT_DIR" \
    --filesystem="$CARGO_HOME_DIR" \
    --env=PROJECT_DIR="$PROJECT_DIR" \
    --env=CARGO_HOME="$CARGO_HOME_DIR" \
    --env=CARGO_TARGET_DIR="$TARGET_DIR" \
    --env=CARGO_RELEASE_FLAG="$CARGO_RELEASE_FLAG" \
    --env=PATH="/usr/lib/sdk/rust-stable/bin:/app/bin:/usr/bin" \
    --command=sh org.gnome.Sdk//50 \
    -lc 'cd "$PROJECT_DIR" && cargo build --locked $CARGO_RELEASE_FLAG'

echo "==> run photo-viewer in app sandbox..."
fix_flatpak_pulse_socket

RUN_CMD=(
    flatpak run
)

if (( RUN_WITH_AUDIO == 1 )); then
    RUN_CMD+=(--socket=pulseaudio)
fi

RUN_CMD+=(
    --env=RUST_LOG="$RUST_LOG_VALUE"
    --filesystem="$PROJECT_DIR"
    --filesystem=home
    --command="$PROJECT_DIR/$TARGET_DIR/$CARGO_PROFILE/photo-viewer"
    "$FLATPAK_APP_ID"
)

exec "${RUN_CMD[@]}"
