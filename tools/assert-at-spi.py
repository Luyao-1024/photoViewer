#!/usr/bin/env python3
"""Assert the accessibility semantics exposed by Photo Viewer's Search page.

This is deliberately a small black-box probe: it connects to the desktop
AT-SPI registry after the real Flatpak window has received Ctrl+F.  It verifies
the user-visible controls that a screen reader needs to navigate the Search
page, rather than treating a reachable accessibility bus as sufficient.

The Search field switch is translated, so the probe resolves the labels it
expects from the same catalogues the app reads.  Freezing one language here
would make the smoke fail in a correctly-rendered English session.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from collections.abc import Iterator
from pathlib import Path


APP_ID = "io.github.luyao_1024.photoviewer"
APP_NAMES = {"photo-viewer", "Photo Viewer", APP_ID}
WINDOW_NAMES = ("Photo Viewer", "照片查看器")
SEARCH_FIELD_KEYS = ("search.field.all", "search.field.name", "search.field.date")
# The zh-CN catalogue values these keys had when the probe was written; used only
# if the checkout's catalogues cannot be read.
FALLBACK_SEARCH_FIELD_LABELS = ("全部", "文件名", "日期")


def normalize_locale(value: str) -> str:
    lowered = value.replace("_", "-").lower()
    if lowered.startswith("zh"):
        return "zh-CN"
    if lowered.startswith("en"):
        return "en"
    return ""


def configured_locale() -> str:
    """Mirror src/core/i18n.rs: config file, then env, then English."""
    home = Path.home()
    config_dirs = []
    xdg_config_home = os.environ.get("XDG_CONFIG_HOME", "")
    if os.path.isabs(xdg_config_home):
        config_dirs.append(Path(xdg_config_home) / APP_ID)
    config_dirs += [
        home / ".config" / APP_ID,
        # A sandboxed run keeps its config under the per-app Flatpak directory.
        home / ".var" / "app" / APP_ID / "config" / APP_ID,
    ]
    for config_dir in config_dirs:
        try:
            config = json.loads((config_dir / "i18n.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        locale = normalize_locale(str(config.get("locale") or ""))
        if locale:
            return locale

    for variable in ("PHOTO_VIEWER_LOCALE", "LC_ALL", "LANG", "LANGUAGE"):
        locale = normalize_locale(os.environ.get(variable, ""))
        if locale:
            return locale
    return "en"


def search_field_labels(locale: str) -> tuple[str, ...]:
    catalogue_path = Path(__file__).resolve().parent.parent / "i18n" / f"{locale}.json"
    try:
        catalogue = json.loads(catalogue_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        print(
            f"warning: cannot read {catalogue_path} ({error}); "
            f"asserting the built-in {FALLBACK_SEARCH_FIELD_LABELS} labels",
            file=sys.stderr,
        )
        return FALLBACK_SEARCH_FIELD_LABELS

    labels = tuple(catalogue.get(key, "") for key in SEARCH_FIELD_KEYS)
    if not all(labels):
        missing = [
            key for key, label in zip(SEARCH_FIELD_KEYS, labels) if not label
        ]
        raise AssertionError(f"{catalogue_path} is missing {', '.join(missing)}")
    return labels


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Assert Photo Viewer Search-page roles, names, and focus through AT-SPI."
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=10.0,
        help="seconds to wait for the Search accessibility tree (default: 10)",
    )
    parser.add_argument(
        "--locale",
        choices=("zh-CN", "en"),
        default=None,
        help="catalogue to read the expected toggle-button names from "
        "(default: resolved the way the app resolves its locale)",
    )
    parser.add_argument(
        "--dump",
        action="store_true",
        help="print the matching application accessibility subtree before asserting",
    )
    return parser.parse_args()


def safe_value(getter, fallback: str = "<unavailable>") -> str:
    try:
        value = getter()
    except Exception:  # AT-SPI nodes can disappear while their app shuts down.
        return fallback
    return str(value) if value else "<unnamed>"


def walk(node, max_depth: int = 20) -> Iterator[tuple[object, int]]:
    """Yield an AT-SPI subtree without failing if a transient child vanishes."""
    pending = [(node, 0)]
    while pending:
        current, depth = pending.pop()
        yield current, depth
        if depth >= max_depth:
            continue
        try:
            child_count = current.getChildCount()
        except Exception:
            continue
        for index in range(child_count - 1, -1, -1):
            try:
                pending.append((current[index], depth + 1))
            except Exception:
                continue


def role_is(node, role) -> bool:
    try:
        return node.getRole() == role
    except Exception:
        return False


def name_is(node, expected: str) -> bool:
    return safe_value(lambda: node.name, "") == expected


def has_state(node, state) -> bool:
    try:
        return state in node.getState().getStates()
    except Exception:
        return False


def application_has_window(app, pyatspi) -> bool:
    return any(
        role_is(node, pyatspi.ROLE_FRAME) and any(name_is(node, name) for name in WINDOW_NAMES)
        for node, _ in walk(app)
    )


def find_photo_viewer_application(desktop, pyatspi):
    applications = [node for node, _ in walk(desktop, max_depth=1) if role_is(node, pyatspi.ROLE_APPLICATION)]
    for app in applications:
        if safe_value(lambda: app.name, "") in APP_NAMES:
            return app
    for app in applications:
        if application_has_window(app, pyatspi):
            return app
    names = ", ".join(sorted(safe_value(lambda app=app: app.name) for app in applications))
    raise AssertionError(f"Photo Viewer application is not present; applications: {names or '<none>'}")


def tree_lines(app) -> list[str]:
    return [
        f"{'  ' * depth}{safe_value(node.getRoleName)}: {safe_value(lambda: node.name)}"
        for node, depth in walk(app)
    ]


def assert_search_page(app, pyatspi, field_names: tuple[str, ...]) -> None:
    nodes = [node for node, _ in walk(app)]

    if not any(
        role_is(node, pyatspi.ROLE_FRAME) and any(name_is(node, name) for name in WINDOW_NAMES)
        for node in nodes
    ):
        expected = " or ".join(repr(name) for name in WINDOW_NAMES)
        raise AssertionError(f"missing localized application frame named {expected}")

    for key, field_name in zip(SEARCH_FIELD_KEYS, field_names):
        if not any(
            role_is(node, pyatspi.ROLE_TOGGLE_BUTTON) and name_is(node, field_name)
            for node in nodes
        ):
            raise AssertionError(
                f"missing toggle button named {field_name!r} (i18n key {key})"
            )

    if not any(
        role_is(node, pyatspi.ROLE_ENTRY) and has_state(node, pyatspi.STATE_FOCUSED)
        for node in nodes
    ):
        raise AssertionError("missing focused entry control for Search")


def main() -> int:
    args = parse_args()
    if args.timeout <= 0:
        raise SystemExit("--timeout must be greater than zero")

    try:
        import pyatspi
    except ImportError as error:
        print(
            "Missing Python AT-SPI client. Install python3-pyatspi (Ubuntu) or python3-pyatspi (Fedora).",
            file=sys.stderr,
        )
        raise SystemExit(1) from error

    locale = args.locale or configured_locale()
    field_names = search_field_labels(locale)

    deadline = time.monotonic() + args.timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            application = find_photo_viewer_application(pyatspi.Registry.getDesktop(0), pyatspi)
            if args.dump:
                print("\n".join(tree_lines(application)))
            assert_search_page(application, pyatspi, field_names)
        except Exception as error:
            last_error = error
            time.sleep(0.25)
            continue

        print(
            f"AT-SPI Search semantics verified for locale {locale}: localized application frame, "
            f"focused Search entry, and {'/'.join(field_names)} toggle buttons."
        )
        return 0

    print(f"AT-SPI Search semantics check failed: {last_error}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
