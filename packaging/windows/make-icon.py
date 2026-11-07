#!/usr/bin/env python3
"""Wrap the unchanged Flatpak PNGs in a Windows ICO container (no image redraw)."""
from pathlib import Path
import struct

pkg = Path(__file__).resolve().parents[1]
images = [(size, (pkg / f'flatpak/icons/{size}x{size}/app.freedombooks.FreedomBooks.png').read_bytes()) for size in (128, 256)]
offset = 6 + 16 * len(images)
entries = []
for size, data in images:
    entries.append(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
    offset += len(data)
target = pkg / 'windows/FreedomBooks.ico'
target.write_bytes(struct.pack('<HHH', 0, 1, len(images)) + b''.join(entries) + b''.join(data for _, data in images))
print(f'ICON CREATED {target}')
