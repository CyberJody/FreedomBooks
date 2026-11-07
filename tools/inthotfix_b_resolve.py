"""INT-HOTFIX part B: resolve the desktop-backups merge conflicts (keep both sides).

Run from the worktree root during the merge of desktop-backups c226b71 into int-hotfix.
Every conflict keeps HEAD (desktop-jody1 / part A) and then the desktop-backups side, except
screens.rs's shell banner: the backups side's `banner` (restored-book banner) is kept, and its
`fb_feedback_url` field is dropped because desktop-jody1 removed that field from `Shell`.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CONFLICT = re.compile(r"<<<<<<< HEAD\n(.*?)=======\n(.*?)>>>>>>> c226b71\n", re.S)


def keep_both(sep=""):
    return lambda ours, theirs: ours + sep + theirs


def resolve(rel, chooser, expect):
    path = ROOT / rel
    text = path.read_text(encoding="utf-8")
    found = CONFLICT.findall(text)
    assert len(found) == expect, (rel, len(found))
    text = CONFLICT.sub(lambda m: chooser(m.group(1), m.group(2)), text)
    assert "<<<<<<<" not in text and ">>>>>>>" not in text, rel
    path.write_text(text, encoding="utf-8")
    print(f"resolved {rel}: {expect} hunk(s)")


def screens_shell(ours, theirs):
    assert ours == "        fb_banner: ALPHA_BANNER.into(),\n", ours
    assert theirs == "        fb_banner: banner,\n        fb_feedback_url: String::new(),\n", theirs
    return "        fb_banner: banner,\n"


def waves(ours, theirs):
    old = ("Screens that intentionally differ from the Django reference. Each is normalized in the diff by an exact\n"
           "Django-side rewrite (never a Rust-side one), listed in `crates/fb-screendiff/NORMALIZATION.md`, and tested to\n"
           "DIFF on any other change.\n")
    new = ("Screens that intentionally differ from the Django reference. Each is normalized in the diff by a narrow rule\n"
           "listed in `crates/fb-screendiff/NORMALIZATION.md` (N13 is an exact Django-side rewrite, never a Rust-side one;\n"
           "the Backups rule replaces four named spans on both sides) and tested to DIFF on any other change.\n")
    assert old in ours
    return ours.replace(old, new) + "\n" + theirs


def main():
    resolve("DESKTOP-WAVES.md", waves, 1)
    resolve("crates/fb-screendiff/NORMALIZATION.md", keep_both("\n"), 1)
    resolve("crates/fb-screendiff/src/main.rs", keep_both(), 1)
    resolve("crates/fb-screendiff/src/normalize.rs", keep_both(), 1)
    resolve("crates/fb-app/src/session.rs", keep_both(), 2)
    resolve("crates/fb-app/src/screens.rs", screens_shell, 1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
