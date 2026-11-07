"""Remove a synthetic HOME without walking into document-portal mounts."""
import os
from pathlib import Path
import shutil
import subprocess


def mounted_below(root):
    def unescape(value):
        for encoded, decoded in [('\\040', ' '), ('\\011', '\t'), ('\\012', '\n'), ('\\134', '\\')]:
            value = value.replace(encoded, decoded)
        return value
    return {Path(unescape(line.split()[4])) for line in Path('/proc/self/mountinfo').read_text().splitlines()
            if Path(unescape(line.split()[4])).is_relative_to(root)}


def remove_unmounted(root, mounts):
    if root in mounts:
        print(f'CLEANUP: safely skipping test-owned mount {root}', flush=True)
        return False
    retained = False
    with os.scandir(root) as entries:
        for entry in entries:
            child = Path(entry.path)
            if child in mounts:
                print(f'CLEANUP: safely skipping test-owned mount {child}', flush=True)
                retained = True
            elif entry.is_dir(follow_symlinks=False):
                retained = not remove_unmounted(child, mounts) or retained
            else:
                child.unlink()
    if not retained:
        root.rmdir()
    return not retained


def cleanup_home(root):
    root = root.resolve()
    for mount in sorted(mounted_below(root), key=lambda p: len(p.parts), reverse=True):
        # Only detach mounts inside this run's synthetic HOME. Never touch the
        # login session's portal, or follow the mount into documents it exposes.
        for command in [('fusermount3', '-u', '-z'), ('fusermount', '-u', '-z'), ('umount', '-l')]:
            if shutil.which(command[0]):
                result = subprocess.run([*command, str(mount)], capture_output=True)
                if result.returncode == 0:
                    break
    remove_unmounted(root, mounted_below(root))
