#!/usr/bin/env bash
# Run a command with a private, verified AT-SPI accessibility session.
#
# Xvfb supplies a display but not a user-session D-Bus. GTK's AT-SPI backend
# needs that bus and the AT-SPI registry, so headless tests must start both
# before initializing GTK.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT_PATH="$SCRIPT_DIR/$(basename -- "${BASH_SOURCE[0]}")"

usage() {
    cat <<'USAGE'
Usage: tools/with-at-spi.sh [--check] <command> [args...]

Start (or use) a session D-Bus, verify org.a11y.Bus and the AT-SPI registry,
then run the command in that same session.

Options:
  --check    Verify the accessibility services and exit without a command.
  -h, --help Show this help.
USAGE
}

require_command() {
    local command_name="$1"
    local package_hint="$2"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "Missing dependency: $command_name ($package_hint)" >&2
        exit 1
    fi
}

verify_at_spi() {
    local bus_response
    local at_spi_address
    local registry_names

    bus_response="$(gdbus call --session \
        --dest org.a11y.Bus \
        --object-path /org/a11y/bus \
        --method org.a11y.Bus.GetAddress)"
    at_spi_address="$(printf '%s\n' "$bus_response" | awk -F"'" 'NF >= 3 { print $2; exit }')"
    if [[ -z "$at_spi_address" ]]; then
        echo "AT-SPI bus returned an invalid address: $bus_response" >&2
        exit 1
    fi

    gdbus call --address "$at_spi_address" \
        --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.StartServiceByName \
        org.a11y.atspi.Registry 0 >/dev/null
    registry_names="$(gdbus call --address "$at_spi_address" \
        --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.ListNames)"
    if [[ "$registry_names" != *"org.a11y.atspi.Registry"* ]]; then
        echo "AT-SPI registry did not register on the accessibility bus" >&2
        exit 1
    fi
}

at_spi_bus_is_reachable() {
    local bus_response
    local at_spi_address

    bus_response="$(gdbus call --session \
        --dest org.a11y.Bus \
        --object-path /org/a11y/bus \
        --method org.a11y.Bus.GetAddress 2>/dev/null)" || return 1
    at_spi_address="$(printf '%s\n' "$bus_response" | awk -F"'" 'NF >= 3 { print $2; exit }')"
    [[ -n "$at_spi_address" ]] || return 1
    gdbus call --address "$at_spi_address" \
        --dest org.freedesktop.DBus \
        --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.ListNames >/dev/null 2>&1
}

if [[ "${1:-}" != "--inside-session" && "${PHOTO_VIEWER_AT_SPI_SESSION_READY:-}" != "1" ]]; then
    # CI containers and remote shells can inherit stale session or AT-SPI bus
    # addresses. Verify the accessibility bus itself before reusing either.
    if ! command -v gdbus >/dev/null 2>&1 || ! at_spi_bus_is_reachable; then
        require_command dbus-run-session "dbus-daemon"
        exec dbus-run-session -- "$SCRIPT_PATH" --inside-session "$@"
    fi
fi

if [[ "${1:-}" == "--inside-session" ]]; then
    shift
fi

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
esac

CHECK_ONLY=0
if [[ "${1:-}" == "--check" ]]; then
    CHECK_ONLY=1
    shift
fi

if (( CHECK_ONLY == 0 && $# == 0 )); then
    usage >&2
    exit 2
fi
if (( CHECK_ONLY )) && (( $# != 0 )); then
    echo "--check cannot be combined with a command" >&2
    exit 2
fi

require_command gdbus "glib2 / libglib2.0-bin"
verify_at_spi
echo "AT-SPI bus and registry ready."

if (( CHECK_ONLY )); then
    exit 0
fi

export PHOTO_VIEWER_AT_SPI_SESSION_READY=1
exec "$@"
