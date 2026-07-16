#!/usr/bin/env python3
"""Assert the accessibility semantics exposed by Photo Viewer's Search page.

This is deliberately a small black-box probe: it connects to the desktop
AT-SPI registry after the real Flatpak window has received Ctrl+F.  It verifies
the user-visible controls that a screen reader needs to navigate the Search
page, rather than treating a reachable accessibility bus as sufficient.
"""

from __future__ import annotations

import argparse
import sys
import time
from collections.abc import Iterator


APP_NAMES = {"photo-viewer", "Photo Viewer", "io.github.luyao_1024.photoviewer"}
WINDOW_NAMES = ("Photo Viewer", "照片查看器")
SEARCH_FIELD_NAMES = ("全部", "文件名", "日期")


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


def assert_search_page(app, pyatspi) -> None:
    nodes = [node for node, _ in walk(app)]

    if not any(
        role_is(node, pyatspi.ROLE_FRAME) and any(name_is(node, name) for name in WINDOW_NAMES)
        for node in nodes
    ):
        expected = " or ".join(repr(name) for name in WINDOW_NAMES)
        raise AssertionError(f"missing localized application frame named {expected}")

    for field_name in SEARCH_FIELD_NAMES:
        if not any(
            role_is(node, pyatspi.ROLE_TOGGLE_BUTTON) and name_is(node, field_name)
            for node in nodes
        ):
            raise AssertionError(f"missing toggle button named '{field_name}'")

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

    deadline = time.monotonic() + args.timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            application = find_photo_viewer_application(pyatspi.Registry.getDesktop(0), pyatspi)
            if args.dump:
                print("\n".join(tree_lines(application)))
            assert_search_page(application, pyatspi)
        except Exception as error:
            last_error = error
            time.sleep(0.25)
            continue

        print(
            "AT-SPI Search semantics verified: localized application frame, focused Search entry, "
            "and 全部/文件名/日期 toggle buttons."
        )
        return 0

    print(f"AT-SPI Search semantics check failed: {last_error}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
