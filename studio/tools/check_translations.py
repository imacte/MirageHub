"""Check every supported translation of the strings Studio shows.

Strings are written in English in the code, inside ``t("...")`` and
``tf("...")``; the tables in ``core/src`` pair each with its translation. This lists any
string with no pair, and any pair whose string is no longer used, and exits
non-zero if there are either -- so a new string cannot ship in English only.

    python studio/tools/check_translations.py
"""
from __future__ import annotations

import pathlib
import re
import sys

STUDIO = pathlib.Path(__file__).resolve().parent.parent
CALL = re.compile(r'\b(?:t|tf)\(\s*"((?:[^"\\]|\\.)*)"')
PAIR = re.compile(r'^\s*\("((?:[^"\\]|\\.)*)",', re.M)
TRANSLATIONS = {"ru.rs": "Russian", "zh_cn.rs": "Simplified Chinese"}
#: Key names the macro editor passes through ``t`` by variable, not literally.
KEY_NAMES = ("Mute", "Volume down", "Volume up", "Next track", "Previous track",
             "Play/Pause", "Right Ctrl", "Right Shift", "Right Alt")


def used() -> list:
    found, seen = [], set()
    for crate in ("app", "core", "setup"):
        for path in sorted((STUDIO / crate / "src").rglob("*.rs")):
            if path.name in TRANSLATIONS:
                continue
            for match in CALL.finditer(path.read_text(encoding="utf-8")):
                if match.group(1) not in seen:
                    seen.add(match.group(1))
                    found.append(match.group(1))
    for name in KEY_NAMES:
        if name not in seen:
            seen.add(name)
            found.append(name)
    return found


def main() -> int:
    strings = used()
    failed = False
    for filename, language in TRANSLATIONS.items():
        translated = set(PAIR.findall((STUDIO / "core" / "src" / filename).read_text(encoding="utf-8")))
        missing = [s for s in strings if s not in translated]
        # Printed key names are listed so they stay untranslated on purpose.
        unused = sorted(s for s in translated - set(strings)
                        if s not in ("Enter", "Esc", "Tab", "Backspace", "Caps Lock", "Insert", "End",
                                     "Page Up", "Page Down", "Print Screen"))
        for text in missing:
            print("no {0} for: {1}".format(language, text))
        for text in unused:
            print("{0} translated but not used: {1}".format(language, text))
        print("{0}: {1} strings, {2} missing, {3} unused".format(language, len(strings), len(missing), len(unused)))
        failed |= bool(missing or unused)
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
