#!/usr/bin/env python3
"""Stop the QEMU guest and isolated HDC server recorded by boot.py."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess

from common import hdc_command, hdc_environment, qmp_command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--guest', type=Path, required=True)
    args = parser.parse_args()
    if not args.guest.is_file():
        return
    guest = json.loads(args.guest.read_text())
    try:
        if Path(guest['qmp']).exists():
            try:
                qmp_command(guest['qmp'], 'quit')
            except (FileNotFoundError, ConnectionRefusedError):
                pass  # Failed boot may already have stopped QEMU.
    finally:
        subprocess.run([*hdc_command(guest), 'kill'], env=hdc_environment(guest), timeout=15, check=True)
    # Keep the control socket available for a retry if QMP cleanup failed.
    shutil.rmtree(guest['qmpDirectory'], ignore_errors=True)


if __name__ == '__main__':
    main()
